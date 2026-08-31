// APCore Protocol — Built-in execution pipeline steps
// Spec reference: design-execution-pipeline.md (Section 3)

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;

use crate::context::Identity;
use crate::errors::{ErrorCode, ModuleError};
use crate::events::emitter::ApCoreEvent;
use crate::executor::{has_schema, redact_sensitive, validate_against_schema};
use crate::pipeline::{BuiltinGate, ExecutionStrategy, PipelineContext, Step, StepResult};
use crate::policy::PolicyDecision;

// Macro for step metadata — execute is implemented manually per step.
macro_rules! step_meta {
    ($name:ident, $step_name:expr, $desc:expr, removable=$rm:expr, replaceable=$rp:expr, pure=$pure:expr) => {
        pub struct $name;

        impl $name {
            fn _name(&self) -> &str {
                $step_name
            }
            fn _description(&self) -> &str {
                $desc
            }
            fn _removable(&self) -> bool {
                $rm
            }
            fn _replaceable(&self) -> bool {
                $rp
            }
            fn _pure(&self) -> bool {
                $pure
            }
        }
    };
}

// Shared helper macro to implement the non-execute Step trait methods.
macro_rules! impl_step_meta {
    ($name:ident) => {
        fn name(&self) -> &str {
            self._name()
        }
        fn description(&self) -> &str {
            self._description()
        }
        fn removable(&self) -> bool {
            self._removable()
        }
        fn replaceable(&self) -> bool {
            self._replaceable()
        }
        fn pure(&self) -> bool {
            self._pure()
        }
    };
}

step_meta!(
    BuiltinContextCreation,
    "context_creation",
    "Create or inherit execution context",
    removable = false,
    replaceable = false,
    pure = true
);
step_meta!(
    BuiltinCallChainGuard,
    "call_chain_guard",
    "Validate call depth and module repeat limits",
    removable = true,
    replaceable = true,
    pure = true
);
/// Resolve a module from the registry by ID and reject disabled modules.
///
/// Unlike the other unit-struct steps, this step carries a per-instance
/// `Arc<ToggleState>` (issue #71). The strategy factory injects the owning
/// `APCore` instance's toggle store so that toggling a module on one instance
/// does not affect another instance in the same process. When no instance
/// handle is available (e.g. `build_standard_strategy()` called directly, or a
/// custom strategy built without an `APCore`), it falls back to the
/// process-global toggle store via [`Default`].
pub struct BuiltinModuleLookup {
    toggle_state: std::sync::Arc<crate::sys_modules::ToggleState>,
}

impl BuiltinModuleLookup {
    /// Build a lookup step bound to a specific toggle store (read path).
    #[must_use]
    pub fn new(toggle_state: std::sync::Arc<crate::sys_modules::ToggleState>) -> Self {
        Self { toggle_state }
    }
}

impl Default for BuiltinModuleLookup {
    /// Fall back to the process-global toggle store, preserving back-compat for
    /// callers that construct the standard strategy without an `APCore`.
    fn default() -> Self {
        Self::new(crate::sys_modules::global_toggle_state_arc())
    }
}
step_meta!(
    BuiltinACLCheck,
    "acl_check",
    "Enforce access control rules",
    removable = true,
    replaceable = true,
    pure = true
);
step_meta!(
    BuiltinApprovalGate,
    "approval_gate",
    "Handle human or AI approval flow",
    removable = true,
    replaceable = true,
    pure = false
);
step_meta!(
    BuiltinInputValidation,
    "input_validation",
    "Validate inputs against schema",
    removable = true,
    replaceable = true,
    pure = true
);
step_meta!(
    BuiltinMiddlewareBefore,
    "middleware_before",
    "Execute before-middleware chain",
    removable = true,
    replaceable = false,
    pure = false
);
step_meta!(
    BuiltinExecute,
    "execute",
    "Invoke the module with timeout",
    removable = false,
    replaceable = true,
    pure = false
);
step_meta!(
    BuiltinOutputValidation,
    "output_validation",
    "Validate outputs against schema",
    removable = true,
    replaceable = true,
    pure = true
);
step_meta!(
    BuiltinMiddlewareAfter,
    "middleware_after",
    "Execute after-middleware chain",
    removable = true,
    replaceable = false,
    pure = false
);
step_meta!(
    BuiltinReturnResult,
    "return_result",
    "Finalize and return output",
    removable = false,
    replaceable = false,
    pure = true
);

// ---------------------------------------------------------------------------
// Step implementations
// ---------------------------------------------------------------------------

#[async_trait]
impl Step for BuiltinContextCreation {
    impl_step_meta!(BuiltinContextCreation);
    fn provides(&self) -> &[&str] {
        &["context"]
    }

    async fn execute(&self, ctx: &mut PipelineContext) -> Result<StepResult, ModuleError> {
        // If the context has no caller_id, default to @external. Preserve
        // every other field on the incoming context (cancel_token,
        // global_deadline, data, identity, services, redacted_inputs/output)
        // so per-call resources like cancellation tokens flow through the
        // pipeline unmodified (otherwise D-21 cancel-token checks observe a
        // fresh, never-cancelled token).
        if ctx.context.caller_id.is_none() {
            ctx.context.caller_id = Some("@external".to_string());
            if ctx.context.identity.is_none() {
                ctx.context.identity = Some(Identity::new(
                    "@external".to_string(),
                    "external".to_string(),
                    vec![],
                    HashMap::new(),
                ));
            }
        }

        // Spec: BuiltinContextCreation MUST set context.global_deadline if
        // the context does not already carry one and the executor config
        // declares a non-zero global_timeout (sync finding A-D-201).
        // Cross-language parity with apcore-python (sets in step 1) and
        // apcore-typescript (sets via Context constructor).
        //
        // Units: BuiltinExecute reads global_deadline as fractional seconds
        // since UNIX_EPOCH (see builtin_steps.rs:500-507). We store the same
        // unit here so set+read are consistent.
        if ctx.context.global_deadline.is_none() {
            if let Some(config) = ctx.config.as_ref() {
                let global_timeout_ms = config.executor.global_timeout;
                if global_timeout_ms > 0 {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs_f64();
                    #[allow(clippy::cast_precision_loss)]
                    let deadline = now + (global_timeout_ms as f64) / 1000.0;
                    ctx.context.global_deadline = Some(deadline);
                }
            }
        }

        // Derive the child context for THIS call. `Context::child` appends
        // `module_id` to `call_chain` and promotes the previous tail to
        // `caller_id`.
        //
        // This MUST live in `context_creation`, which is non-removable, not in
        // `call_chain_guard`, which `build_testing_strategy` and
        // `build_minimal_strategy` remove. With the derivation in the guard,
        // those two presets never grew `call_chain`, so depth limits,
        // circular-call detection and frequency throttling all reset and a
        // nested call reported the wrong `caller_id`. apcore-python
        // (`builtin_steps.py` context_creation) and apcore-typescript
        // (`builtin-steps.ts` contextCreation) both derive it here;
        // docs/spec/design-execution-pipeline.md §4.0 lists `context_creation`
        // among the mandatory non-removable steps for exactly this reason.
        ctx.context = ctx.context.child(&ctx.module_id);

        Ok(StepResult::continue_step())
    }
}

#[async_trait]
impl Step for BuiltinCallChainGuard {
    impl_step_meta!(BuiltinCallChainGuard);
    fn requires(&self) -> &[&str] {
        &["context"]
    }

    async fn execute(&self, ctx: &mut PipelineContext) -> Result<StepResult, ModuleError> {
        // INVARIANT: Executor::inject_resources sets config before any step runs.
        let config = ctx
            .config
            .as_ref()
            .expect("config must be injected into PipelineContext");
        // Cancellation check before any expensive validation/middleware work
        // (D-21 — `Cancel Token Mid-Pipeline Check`, point 1). The pipeline
        // MUST observe `cancel_token` at the call-chain guard so a caller
        // who cancelled before submission does not pay the cost of ACL,
        // middleware, and validation steps. Spec:
        // docs/features/core-executor.md §Cancel Token Mid-Pipeline Check.
        if let Some(token) = ctx.context.cancel_token.as_ref() {
            token.check_for(&ctx.module_id)?;
        }
        // The child context was already derived by the non-removable
        // `context_creation` step, so `call_chain` here already ends with
        // `module_id`. The guard counts the full chain — including the
        // trailing self-entry — for frequency, and strips it for circular
        // detection (sync findings A-D-039 / A-D-040). Deriving it here
        // instead would tie chain growth to a removable step; see the note in
        // `BuiltinContextCreation::execute`.
        crate::utils::guard_call_chain_with_repeat(
            &ctx.context,
            &ctx.module_id,
            config.executor.max_call_depth,
            config.executor.max_module_repeat as usize,
        )?;
        Ok(StepResult::continue_step())
    }
}

#[async_trait]
impl Step for BuiltinModuleLookup {
    fn name(&self) -> &'static str {
        "module_lookup"
    }
    fn description(&self) -> &'static str {
        "Resolve module from registry by ID"
    }
    fn removable(&self) -> bool {
        false
    }
    fn replaceable(&self) -> bool {
        false
    }
    fn pure(&self) -> bool {
        true
    }
    fn provides(&self) -> &[&str] {
        &["module"]
    }

    async fn execute(&self, ctx: &mut PipelineContext) -> Result<StepResult, ModuleError> {
        // INVARIANT: Executor::inject_resources sets registry before any step runs.
        let registry = ctx
            .registry
            .as_ref()
            .expect("registry must be injected into PipelineContext");
        let module = registry.get(&ctx.module_id)?.ok_or_else(|| {
            ModuleError::new(
                ErrorCode::ModuleNotFound,
                format!("Module '{}' not found in registry", ctx.module_id),
            )
        })?;
        // Check if the module is disabled before proceeding. Two sources:
        //   1. The registry descriptor's `enabled` flag (direct registry-level
        //      disable).
        //   2. The per-instance ToggleState owned by the `APCore` that built
        //      this strategy (issue #71). It is the authoritative store written
        //      by that instance's `ToggleFeatureModule` and survives registry
        //      reloads (sync finding A-D-12). The pipeline MUST consult it
        //      because the toggle no longer mutates the descriptor flag, and it
        //      MUST read the instance store (not the process-global) so toggles
        //      stay isolated per `APCore`. Mirrors apcore-python
        //      BuiltinModuleLookup (builtin_steps.py:252).
        if matches!(registry.is_enabled(&ctx.module_id), Some(false))
            || self.toggle_state.is_disabled(&ctx.module_id)
        {
            return Err(ModuleError::new(
                ErrorCode::ModuleDisabled,
                format!("Module '{}' is disabled", ctx.module_id),
            ));
        }
        ctx.module = Some(module.clone());

        // PROTOCOL_SPEC §6.1.8 rule 1: compute the governance projection of the
        // call's arguments HERE, at Step 3, so it is available to the ACL check
        // at Step 4. The ordering is normative, not incidental — the `arguments`
        // condition (§6.1.7) has nothing to read otherwise, and a rule carrying
        // it would load, match, and go unevaluable on every call.
        //
        // Deliberately NOT `context.redacted_inputs`, which is computed just
        // below for a different purpose (§6.1.8 rule 3): that field's contract
        // is safe *logging*, it is a raw copy when the module declares no input
        // schema, and one field serving both "safe to log" and "input to a
        // security decision" will eventually break one of them in a change made
        // for the other. The projection carries the key set and each key's JSON
        // type and has no field that can hold a value.
        ctx.governance_projection = Some(crate::acl::GovernanceProjection::from_arguments(
            &ctx.inputs,
        ));

        // Early input redaction: set context.redacted_inputs BEFORE
        // middleware runs (step 6), so logging sees redacted data.
        let input_schema = module.input_schema().clone();
        if has_schema(&input_schema) {
            let redacted = redact_sensitive(&ctx.inputs, &input_schema);
            ctx.context.redacted_inputs = Some(
                redacted
                    .as_object()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .collect(),
            );
        } else {
            ctx.context.redacted_inputs = Some(
                ctx.inputs
                    .as_object()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .collect(),
            );
        }

        Ok(StepResult::continue_step())
    }
}

#[async_trait]
impl Step for BuiltinACLCheck {
    impl_step_meta!(BuiltinACLCheck);

    /// PROTOCOL_SPEC 6.6.5.2 — the capability marker that identifies this as
    /// the framework's ACL gate, so `governance_state()` never has to match on
    /// the step's name.
    fn builtin_gate(&self) -> Option<BuiltinGate> {
        Some(BuiltinGate::Acl)
    }

    fn requires(&self) -> &[&str] {
        &["context", "module"]
    }

    async fn execute(&self, ctx: &mut PipelineContext) -> Result<StepResult, ModuleError> {
        if let Some(acl) = ctx.acl.clone() {
            let caller_id = ctx.context.caller_id.clone();
            // The structured accessor (PROTOCOL_SPEC §6.8.1), never the boolean:
            // Step 4 now produces TWO results (§6.1.6) and `check()` folds them
            // together by failing closed, which would turn "allowed, ask a
            // human" into a denial here. The Executor is the one caller that
            // must see them apart.
            let decision = acl.check_access(
                caller_id.as_deref(),
                &ctx.module_id,
                Some(&ctx.context),
                ctx.governance_projection.as_ref(),
            );
            // Carried to Step 5 and to `Executor::validate`'s preflight, which
            // union it with the module annotation and the policy (§6.9 rows
            // 3-6). Recorded even on a denial, where it is always false.
            ctx.acl_approval_required = decision.approval_required;
            if !decision.is_allowed() {
                // Publish a governance event on denial (canonical name proposed
                // in apcore#77). Guarded by dry_run so a validate() preflight
                // probe never emits a spurious denial event; fires only on deny
                // (allows are high-volume and already covered by tracing spans).
                if !ctx.dry_run {
                    if let Some(emitter) = ctx.event_emitter.clone() {
                        let event = ApCoreEvent::with_module(
                            "apcore.acl.denied",
                            serde_json::json!({
                                "module_id": ctx.module_id,
                                "caller_id": caller_id,
                                "reason": "ACL denied",
                                "trace_id": ctx.context.trace_id,
                            }),
                            ctx.module_id.clone(),
                            "warn",
                        );
                        emitter.emit(&event).await;
                    }
                }
                return Err(ModuleError::new(
                    ErrorCode::ACLDenied,
                    format!(
                        "Access denied: caller '{:?}' cannot access module '{}'",
                        caller_id, ctx.module_id
                    ),
                ));
            }
        }
        Ok(StepResult::continue_step())
    }
}

impl BuiltinApprovalGate {
    /// Emit an `approval_decision` audit/observability event for a resolved
    /// approval. Mirrors apcore-python (`_emit_audit`) and apcore-typescript:
    /// emits a structured `tracing` log and, when a tracing span stack is
    /// present in `ctx.data` (`TRACING_SPANS`), appends an `approval_decision`
    /// event carrying `{module_id, status, approved_by, reason, approval_id}`
    /// to the active (last) span.
    fn emit_approval_decision(ctx: &PipelineContext, result: &crate::approval::ApprovalResult) {
        let approved_by = result.approved_by.clone().unwrap_or_default();
        let reason = result.reason.clone().unwrap_or_default();
        let approval_id = result.approval_id.clone().unwrap_or_default();

        tracing::info!(
            module_id = %ctx.module_id,
            status = %result.status,
            approved_by = %approved_by,
            reason = %reason,
            approval_id = %approval_id,
            "approval_decision"
        );

        // Append the event to the active tracing span when the span stack is
        // present in context data (cross-SDK parity). No-op when tracing is
        // inactive, matching the reference SDKs.
        if let Some(mut spans) = crate::context_keys::TRACING_SPANS.get(&ctx.context) {
            if let Some(last) = spans.last_mut() {
                if let Some(events) = last.as_object_mut().and_then(|obj| {
                    obj.entry("events")
                        .or_insert_with(|| serde_json::json!([]))
                        .as_array_mut()
                }) {
                    events.push(serde_json::json!({
                        "name": "approval_decision",
                        "module_id": ctx.module_id,
                        "status": result.status,
                        "approved_by": approved_by,
                        "reason": reason,
                        "approval_id": approval_id,
                    }));
                    crate::context_keys::TRACING_SPANS.set(&ctx.context, spans);
                }
            }
        }
    }

    /// Publish `apcore.approval.decision` on the event bus (apcore#77) when an
    /// event emitter is configured. Emitted for every adjudication (handler
    /// decisions AND strict fail-closed rejections). Severity mirrors the
    /// outcome: `approved`/`pending` are `info`; `rejected`/`timeout`
    /// (governance interventions) are `warn`. Emitted ALONGSIDE the log/span.
    async fn emit_approval_decision_event(
        ctx: &PipelineContext,
        result: &crate::approval::ApprovalResult,
    ) {
        let Some(emitter) = ctx.event_emitter.clone() else {
            return;
        };
        let severity = if matches!(result.status.as_str(), "approved" | "pending") {
            "info"
        } else {
            "warn"
        };
        let event = ApCoreEvent::with_module(
            "apcore.approval.decision",
            serde_json::json!({
                "module_id": ctx.module_id,
                "status": result.status,
                "approved_by": result.approved_by,
                "reason": result.reason,
                "approval_id": result.approval_id,
                "trace_id": ctx.context.trace_id,
            }),
            ctx.module_id.clone(),
            severity,
        );
        emitter.emit(&event).await;
    }

    /// Audit a policy-driven override: structured log + active-span event.
    /// Publishes to the bus separately via [`Self::emit_policy_override_event`].
    fn emit_policy_override(ctx: &PipelineContext, decision: &PolicyDecision) {
        let pattern = decision.rule.as_ref().map_or("", |r| r.pattern());
        let reason = decision
            .rule
            .as_ref()
            .and_then(|r| r.reason())
            .unwrap_or_default();

        tracing::info!(
            module_id = %decision.module_id,
            pattern = %pattern,
            requires_approval = decision.requires_approval,
            destructive = decision.destructive,
            needs_approval = decision.needs_approval,
            reason = %reason,
            "policy_override"
        );

        if let Some(mut spans) = crate::context_keys::TRACING_SPANS.get(&ctx.context) {
            if let Some(last) = spans.last_mut() {
                if let Some(events) = last.as_object_mut().and_then(|obj| {
                    obj.entry("events")
                        .or_insert_with(|| serde_json::json!([]))
                        .as_array_mut()
                }) {
                    events.push(serde_json::json!({
                        "name": "policy_override",
                        "module_id": decision.module_id,
                        "pattern": pattern,
                        "requires_approval": decision.requires_approval,
                        "destructive": decision.destructive,
                        "needs_approval": decision.needs_approval,
                        "reason": reason,
                    }));
                    crate::context_keys::TRACING_SPANS.set(&ctx.context, spans);
                }
            }
        }
    }

    /// Publish `apcore.policy.override` on the event bus (apcore#77) when an
    /// event emitter is configured. Emitted whenever a policy rule/gate changed
    /// a base governance value (`PolicyDecision::overridden`).
    async fn emit_policy_override_event(ctx: &PipelineContext, decision: &PolicyDecision) {
        let Some(emitter) = ctx.event_emitter.clone() else {
            return;
        };
        let pattern = decision.rule.as_ref().map_or("", |r| r.pattern());
        let reason = decision
            .rule
            .as_ref()
            .and_then(|r| r.reason())
            .unwrap_or_default();
        let event = ApCoreEvent::with_module(
            "apcore.policy.override",
            serde_json::json!({
                "module_id": decision.module_id,
                "pattern": pattern,
                "requires_approval": decision.requires_approval,
                "destructive": decision.destructive,
                "needs_approval": decision.needs_approval,
                "reason": reason,
                "trace_id": ctx.context.trace_id,
            }),
            decision.module_id.clone(),
            "info",
        );
        emitter.emit(&event).await;
    }

    /// Log a fail-loud governance warning once per dedup `key`
    /// (module/kind pair), using the executor-owned dedup set threaded via
    /// `ctx.governance_warned`. Without a set (standalone step) it always warns.
    fn warn_once(ctx: &PipelineContext, key: &str, message: &str) {
        if let Some(warned) = ctx.governance_warned.as_ref() {
            if !warned.lock().insert(key.to_string()) {
                return;
            }
        }
        tracing::warn!("{}", message);
    }
}

#[async_trait]
impl Step for BuiltinApprovalGate {
    impl_step_meta!(BuiltinApprovalGate);

    /// PROTOCOL_SPEC 6.6.5.2 — see [`BuiltinACLCheck::builtin_gate`].
    fn builtin_gate(&self) -> Option<BuiltinGate> {
        Some(BuiltinGate::Approval)
    }
    fn requires(&self) -> &[&str] {
        &["context", "module"]
    }

    #[allow(clippy::too_many_lines)] // approval gate logic is inherently multi-step; splitting would obscure the protocol flow
    async fn execute(&self, ctx: &mut PipelineContext) -> Result<StepResult, ModuleError> {
        // INVARIANT: Executor::inject_resources sets registry before any step runs.
        let registry = ctx
            .registry
            .as_ref()
            .expect("registry must be injected into PipelineContext");

        // Base governance annotations from the registered descriptor. Gate
        // firing is decided from these (the ApprovalRequest metadata, however,
        // is sourced from the live module instance below — PROTOCOL_SPEC §7.4).
        let desc_annotations = registry
            .get_definition(&ctx.module_id)
            .ok()
            .flatten()
            .and_then(|d| d.annotations);

        // Consult the ExecutionPolicy (apcore#76). A matched rule (or
        // gate_destructive) overrides the module's declared governance values.
        //
        // PROTOCOL_SPEC §7.9.6 (v1.24.0, apcore#102): resolution receives the
        // call site — the invocation `arguments` and the `Context` — which are
        // already in scope here because the gate is Step 5. The built-in
        // pattern rules MUST NOT consult them, so no existing verdict moves;
        // the inputs exist so the call site reaches the audit trail (rule 3).
        // Those arguments have NOT been schema-validated: input validation is
        // Step 7. `resolve_with_call_site` strips `_approval_token` before
        // resolving (rule 5) — §7.4's "before passing to subsequent steps" does
        // not reach inside Step 5, so the strip below happens too late for the
        // policy and the API boundary is where it has to be enforced.
        let decision: Option<PolicyDecision> = ctx.policy.clone().map(|policy| {
            policy.resolve_with_call_site(
                &ctx.module_id,
                desc_annotations.as_ref(),
                Some(&ctx.inputs),
                Some(&ctx.context),
            )
        });

        let (policy_effective_approval, effective_destructive) = match &decision {
            Some(d) => (d.needs_approval, d.destructive),
            None => (
                desc_annotations
                    .as_ref()
                    .is_some_and(|a| a.requires_approval),
                desc_annotations.as_ref().is_some_and(|a| a.destructive),
            ),
        };

        // PROTOCOL_SPEC §7.4 / §6.9 rows 3-5: the gate fires on the UNION of
        // the module's `annotations.requires_approval`, the Step-4 ACL
        // decision, and `gate_destructive`. `policy_effective_approval` already
        // folds the annotation and `gate_destructive` together; the ACL's
        // requirement joins them here.
        //
        // The union is what makes §6.9 row 4 hold: an `ExecutionPolicy`
        // override may ADD a requirement and MUST NOT remove one the ACL set.
        // The ACL is a caller-scoped authorization layer and `ExecutionPolicy`
        // is a module-scoped platform override, so letting the module-scoped
        // one cancel the caller-scoped one is a privilege escalation — a policy
        // rule written for `orders.*` would silently strip a requirement an ACL
        // author attached to one untrusted caller. `policy_effective_approval`
        // is ORed, never assigned over, which is the whole guard: a policy
        // `requires_approval: false` overrides the module's *annotation* and
        // never the ACL's decision.
        let acl_requires_approval = ctx.acl_approval_required;
        let needs_approval = policy_effective_approval || acl_requires_approval;

        // Audit a policy-driven override: log, span, and bus event (apcore#77).
        if let Some(d) = &decision {
            if d.overridden {
                Self::emit_policy_override(ctx, d);
                Self::emit_policy_override_event(ctx, d).await;
            }
        }

        if !needs_approval {
            // Fail-loud governance (apcore#76): a module that is effectively
            // destructive but that no approval gate covers is warned about once.
            if effective_destructive {
                Self::warn_once(
                    ctx,
                    &format!("destructive_ungated:{}", ctx.module_id),
                    &format!(
                        "Module '{}' is annotated destructive=true but is not covered by the \
                         approval gate (requires_approval is false and no policy gates \
                         destructive modules). Consider requires_approval=true or \
                         ExecutionPolicy(gate_destructive=true). (apcore#76)",
                        ctx.module_id
                    ),
                );
            }
            return Ok(StepResult::continue_step());
        }

        // The module needs approval. Resolve the handler; when none is
        // configured, FLIP the historical silent-skip to fail-loud (apcore#76).
        let Some(handler) = ctx.approval_handler.clone() else {
            if ctx.policy.as_ref().is_some_and(|p| p.strict) {
                // strict=true fails CLOSED: build a rejected result, audit it,
                // and deny.
                let result = crate::approval::ApprovalResult {
                    status: "rejected".to_string(),
                    reason: Some(
                        "Approval required but no ApprovalHandler is configured; \
                         ExecutionPolicy(strict=true) fails closed"
                            .to_string(),
                    ),
                    ..Default::default()
                };
                Self::emit_approval_decision(ctx, &result);
                Self::emit_approval_decision_event(ctx, &result).await;
                return Err(ModuleError::new(
                    ErrorCode::ApprovalDenied,
                    format!(
                        "Approval denied for module '{}': {}",
                        ctx.module_id,
                        result.reason.unwrap_or_default()
                    ),
                ));
            }
            // Non-strict: keep the PROTOCOL_SPEC §7.4 skip behavior but warn
            // once per module (fail-loud, not silent). Emits no decision event
            // — parity with the "no audit log when gate skipped" contract.
            Self::warn_once(
                ctx,
                &format!("no_handler:{}", ctx.module_id),
                &format!(
                    "Module '{}' requires approval but no ApprovalHandler is configured; \
                     the approval gate is skipped per PROTOCOL_SPEC §7.4. Configure an \
                     approval handler, or ExecutionPolicy(strict=true) to fail closed. \
                     (apcore#76)",
                    ctx.module_id
                ),
            );
            return Ok(StepResult::continue_step());
        };

        // Phase B: check for _approval_token in inputs.
        let approval_result = if let Some(token) = ctx
            .inputs
            .as_object()
            .and_then(|obj| obj.get("_approval_token"))
        {
            let token_str = match token.as_str() {
                Some(s) => s.to_string(),
                None => {
                    return Err(ModuleError::new(
                        ErrorCode::GeneralInvalidInput,
                        format!(
                            "_approval_token must be a string, got {}",
                            match token {
                                serde_json::Value::Number(_) => "number",
                                serde_json::Value::Bool(_) => "boolean",
                                serde_json::Value::Array(_) => "array",
                                serde_json::Value::Object(_) => "object",
                                serde_json::Value::Null => "null",
                                serde_json::Value::String(_) => "string", // covered by as_str() above
                            }
                        ),
                    ));
                }
            };
            // Strip _approval_token from inputs.
            if let Some(obj) = ctx.inputs.as_object_mut() {
                obj.remove("_approval_token");
            }
            handler.check_approval(&token_str).await?
        } else {
            // Spec (PROTOCOL_SPEC §7.4 Step 5): ApprovalRequest MUST carry the
            // resolved live module instance's real annotations / description /
            // tags so handlers can inspect them. Sourced from the live module
            // (`module.annotations()` / `.description()` / `.tags()`) — NOT from
            // the registry descriptor — matching apcore-python
            // (`getattr(module, ...)`) and apcore-typescript (`mod[...]`).
            let module = registry.get(&ctx.module_id)?.ok_or_else(|| {
                ModuleError::new(
                    ErrorCode::ModuleNotFound,
                    format!("Module '{}' not found in registry", ctx.module_id),
                )
            })?;
            let mut annotations = module.annotations();
            // Preserve the ApprovalRequest contract ("requires_approval is
            // guaranteed true", PROTOCOL_SPEC §7) under policy overrides: the
            // handler sees the effective governance values, not the module's
            // raw declaration. Reaching this point means the call needs
            // approval, so requires_approval is true by definition (covers a
            // rule override, gate_destructive, and — since v1.28.0 — an ACL
            // rule carrying `approval`). §7.4 rule 3 makes it explicit: an
            // ACL-sourced requirement makes `requires_approval` effectively
            // true for that call. (apcore#76, apcore#108)
            annotations.requires_approval = true;
            if let Some(d) = &decision {
                annotations.destructive = d.destructive;
            }
            let module_description = module.description();
            let description = if module_description.is_empty() {
                None
            } else {
                Some(module_description.to_string())
            };
            let tags = module.tags();
            let request = crate::approval::ApprovalRequest {
                module_id: ctx.module_id.clone(),
                arguments: ctx.inputs.clone(),
                context: Some(ctx.context.clone()),
                annotations,
                description,
                tags,
            };
            handler.request_approval(&request).await?
        };

        // D10-001: emit an `approval_decision` audit/observability event before
        // the status -> Err mapping below. Cross-SDK parity with apcore-python
        // (`_emit_audit`) and apcore-typescript: log the decision and append an
        // `approval_decision` event to the active tracing span when one exists.
        Self::emit_approval_decision(ctx, &approval_result);
        Self::emit_approval_decision_event(ctx, &approval_result).await;

        match approval_result.status.as_str() {
            "approved" => {}
            "rejected" => {
                return Err(ModuleError::new(
                    ErrorCode::ApprovalDenied,
                    format!(
                        "Approval denied for module '{}': {}",
                        ctx.module_id,
                        approval_result
                            .reason
                            .unwrap_or_else(|| "no reason given".to_string())
                    ),
                ));
            }
            "timeout" => {
                return Err(ModuleError::new(
                    ErrorCode::ApprovalTimeout,
                    format!(
                        "Approval timed out for module '{}': {}",
                        ctx.module_id,
                        approval_result
                            .reason
                            .unwrap_or_else(|| "no reason given".to_string())
                    ),
                ));
            }
            "pending" => {
                // The `approval_id` MUST travel on the error: it is the token a
                // caller needs to resume via `_approval_token` once the external
                // approval lands (protocol-spec.md §7.4 "Resume semantics", and
                // conformance fixture approval_gate.json `gate_fires_pending`).
                // Without it a pending result is a dead end. Mirrors
                // apcore-python `ApprovalPendingError.__init__`, which sets
                // `self.details["approval_id"]`.
                let mut details: std::collections::HashMap<String, serde_json::Value> =
                    std::collections::HashMap::new();
                details.insert(
                    "approval_id".to_string(),
                    match &approval_result.approval_id {
                        Some(approval_id) => serde_json::Value::String(approval_id.clone()),
                        None => serde_json::Value::Null,
                    },
                );
                return Err(ModuleError::new(
                    ErrorCode::ApprovalPending,
                    format!(
                        "Approval pending for module '{}': {}",
                        ctx.module_id,
                        approval_result
                            .reason
                            .clone()
                            .unwrap_or_else(|| "no reason given".to_string())
                    ),
                )
                .with_details(details));
            }
            _ => {
                tracing::warn!(
                    module_id = %ctx.module_id,
                    status = %approval_result.status,
                    "Unknown approval status, treating as denied"
                );
                return Err(ModuleError::new(
                    ErrorCode::ApprovalDenied,
                    format!(
                        "Approval denied for module '{}': unknown status '{}'",
                        ctx.module_id, approval_result.status
                    ),
                ));
            }
        }

        Ok(StepResult::continue_step())
    }
}

#[async_trait]
impl Step for BuiltinInputValidation {
    impl_step_meta!(BuiltinInputValidation);
    fn requires(&self) -> &[&str] {
        &["module"]
    }
    fn provides(&self) -> &[&str] {
        &["validated_inputs"]
    }

    async fn execute(&self, ctx: &mut PipelineContext) -> Result<StepResult, ModuleError> {
        // INVARIANT: BuiltinModuleLookup runs before this step and sets ctx.module.
        let module = ctx
            .module
            .as_ref()
            .expect("module must be resolved before input_validation");
        let input_schema = module.input_schema();
        validate_against_schema(&ctx.inputs, &input_schema, "Input")?;

        // Store redacted inputs on context.
        if has_schema(&input_schema) {
            let redacted = redact_sensitive(&ctx.inputs, &input_schema);
            ctx.context.redacted_inputs = Some(
                redacted
                    .as_object()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .collect(),
            );
        }
        ctx.validated_inputs = Some(ctx.inputs.clone());
        Ok(StepResult::continue_step())
    }
}

#[async_trait]
impl Step for BuiltinMiddlewareBefore {
    impl_step_meta!(BuiltinMiddlewareBefore);

    async fn execute(&self, ctx: &mut PipelineContext) -> Result<StepResult, ModuleError> {
        let middleware_manager = match ctx.middleware_manager {
            Some(ref mm) => mm.clone(),
            None => return Ok(StepResult::continue_step()),
        };

        let (modified_inputs, executed) = match middleware_manager
            .execute_before(&ctx.module_id, ctx.inputs.clone(), &ctx.context)
            .await
        {
            Ok((inputs, executed)) => (inputs, executed),
            Err(e) => {
                // Recover the list of middlewares that ran during execute_before
                // from the MiddlewareChainError details so on_error runs over
                // exactly those (mirrors Python builtin_steps.py:483 setting
                // ctx.executed_middlewares = exc.executed_middlewares and TS
                // builtin-steps.ts:442) — sync finding A-D-01. Without this the
                // recovery middleware's on_error is skipped (empty &[]) and the
                // executor-level on_error/RetrySignal loop is bypassed.
                let executed: Vec<usize> = e
                    .details
                    .get("executed_middlewares")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
                ctx.executed_middlewares.clone_from(&executed);
                // A-D-010 / A-D-012: do NOT run step-level on_error recovery
                // here. Record which middlewares ran (above) and propagate the
                // error so the executor-level loop is the SOLE on_error site —
                // it runs `execute_on_error_outcome` over `executed` and honors
                // both Recovery and Retry. Step-level `execute_on_error` mapped
                // RetrySignal→None (dropping retries) and, combined with the
                // executor pass, fired on_error twice. Mirrors apcore-python
                // (chain errors store executed_middlewares + raise) and
                // apcore-typescript.
                let _ = &middleware_manager; // retained for the Ok-path borrow above
                return Err(e);
            }
        };
        ctx.inputs = modified_inputs;
        ctx.executed_middlewares = executed;
        Ok(StepResult::continue_step())
    }
}

/// Resolve the effective per-call timeout in milliseconds.
///
/// Starts from the module's declared `annotations.extra["resources"]["timeout"]`
/// (spec D-11), falls back to `config.executor.default_timeout`, then clamps
/// against whatever remains of `global_deadline` — the dual-timeout model in
/// docs/features/core-executor.md §Step 8. `0` means "no timeout".
///
/// Shared by `BuiltinExecute` and `Executor::stream`'s non-streaming fallback.
/// The streaming path drives `run_until_step(.., "execute")`, so it never
/// enters `BuiltinExecute`; before this was extracted, that fallback awaited
/// `module.execute()` bare, with neither the per-module timeout nor the
/// deadline clamp, while both peer SDKs run the full pipeline and get both.
pub(crate) fn resolve_effective_timeout_ms(
    registry: Option<&std::sync::Arc<crate::registry::registry::Registry>>,
    module_id: &str,
    global_deadline: Option<f64>,
    default_timeout: u64,
) -> Result<u64, ModuleError> {
    let declared_timeout: Option<serde_json::Value> = registry
        .and_then(|reg| reg.get_definition(module_id).ok().flatten())
        .and_then(|desc| desc.annotations)
        .and_then(|ann| ann.extra.get("resources").cloned())
        .and_then(|res| res.get("timeout").cloned());
    // A negative declared timeout is invalid and MUST be rejected rather than
    // silently swallowed (`as_u64()` would return None and fall back to the
    // default). Spec Edge Cases table; peer Python builtin_steps.py:620 raises
    // InvalidInputError (sync finding A-D-W1).
    if let Some(ref v) = declared_timeout {
        let is_negative = v.as_i64().is_some_and(|n| n < 0) || v.as_f64().is_some_and(|f| f < 0.0);
        if is_negative {
            return Err(ModuleError::new(
                ErrorCode::GeneralInvalidInput,
                format!(
                    "Module '{module_id}' declares a negative timeout ({v}); timeout must be non-negative"
                ),
            ));
        }
    }
    let per_module_timeout_ms: Option<u64> = declared_timeout.and_then(|v| v.as_u64());
    let mut timeout_ms = per_module_timeout_ms.unwrap_or(default_timeout);
    if let Some(deadline) = global_deadline {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        // intentional: remaining_ms is non-negative and fits in u64 for any reasonable deadline
        let remaining_ms = ((deadline - now) * 1000.0) as u64;
        if remaining_ms == 0 {
            return Err(ModuleError::new(
                ErrorCode::ModuleTimeout,
                format!("Module '{module_id}' execution aborted: global deadline already exceeded"),
            ));
        }
        if timeout_ms == 0 || remaining_ms < timeout_ms {
            timeout_ms = remaining_ms;
        }
    }
    Ok(timeout_ms)
}

#[async_trait]
impl Step for BuiltinExecute {
    impl_step_meta!(BuiltinExecute);
    fn requires(&self) -> &[&str] {
        &["module"]
    }
    fn provides(&self) -> &[&str] {
        &["output"]
    }

    async fn execute(&self, ctx: &mut PipelineContext) -> Result<StepResult, ModuleError> {
        // INVARIANT: BuiltinModuleLookup runs before this step and sets ctx.module.
        let module = ctx
            .module
            .as_ref()
            .expect("module must be resolved before execute")
            .clone();
        // INVARIANT: Executor::inject_resources sets config before any step runs.
        let config = ctx
            .config
            .as_ref()
            .expect("config must be injected into PipelineContext");

        // Compute effective timeout — start from per-module override
        // (`annotations.extra["resources"]["timeout"]`, in milliseconds; spec
        // D-11), fall back to `config.executor.default_timeout`. Both are
        // then clamped against the remaining global deadline below.
        // Spec: docs/features/core-executor.md §Step 8 (dual-timeout model).
        let timeout_ms = resolve_effective_timeout_ms(
            ctx.registry.as_ref(),
            &ctx.module_id,
            ctx.context.global_deadline,
            config.executor.default_timeout,
        )?;

        // Note: Streaming is handled exclusively by `Executor::stream()`, which
        // bypasses this step entirely. `ctx.stream` is intentionally ignored
        // here — `BuiltinExecute` always calls `module.execute()` for the
        // unary path. See `Executor::stream` for the streaming pipeline.

        // Cancellation check immediately before invoking the module
        // (D-21 — `Cancel Token Mid-Pipeline Check`, point 2). Acts as a
        // defensive backstop for tokens that became cancelled while earlier
        // steps were running, ensuring `cancel()` interrupts the in-flight
        // pipeline rather than handing control to a doomed module. Spec:
        // docs/features/core-executor.md §Cancel Token Mid-Pipeline Check.
        if let Some(token) = ctx.context.cancel_token.as_ref() {
            token.check_for(&ctx.module_id)?;
        }

        let execute_result = if timeout_ms > 0 {
            match tokio::time::timeout(
                Duration::from_millis(timeout_ms),
                module.execute(ctx.inputs.clone(), &ctx.context),
            )
            .await
            {
                Ok(result) => result,
                Err(_elapsed) => Err(ModuleError::new(
                    ErrorCode::ModuleTimeout,
                    format!(
                        "Module '{}' execution timed out after {}ms",
                        ctx.module_id, timeout_ms
                    ),
                )),
            }
        } else {
            module.execute(ctx.inputs.clone(), &ctx.context).await
        };

        match execute_result {
            Ok(output) => {
                ctx.output = Some(output);
                Ok(StepResult::continue_step())
            }
            Err(e) => {
                // Cancellation short-circuit (D-20): an ExecutionCancelled
                // error MUST bypass the on_error middleware chain even at the
                // step level, so a recovering on_error middleware cannot
                // swallow it into a success before reaching the executor-level
                // guard in `Executor::call` / `call_with_trace`. Propagate it
                // untouched. Mirrors apcore-python `_handle_error`
                // (`_unwrap_cancellation` before any on_error recovery).
                // Spec: docs/features/core-executor.md §Cancellation Short-Circuit
                if matches!(e.code, ErrorCode::ExecutionCancelled) {
                    return Err(e);
                }
                // A-D-010 / A-D-012: do NOT run step-level on_error recovery
                // here. The executor-level loop is the sole recovery site (it
                // runs `execute_on_error_outcome` over `ctx.executed_middlewares`
                // and honors Recovery + Retry). Step-level recovery double-fired
                // on_error and silently dropped RetrySignals. Mirrors
                // apcore-python `BuiltinExecute` (raises; no step-level on_error)
                // and apcore-typescript.
                Err(e)
            }
        }
    }
}

#[async_trait]
impl Step for BuiltinOutputValidation {
    impl_step_meta!(BuiltinOutputValidation);
    fn requires(&self) -> &[&str] {
        &["module", "output"]
    }
    fn provides(&self) -> &[&str] {
        &["validated_output"]
    }

    async fn execute(&self, ctx: &mut PipelineContext) -> Result<StepResult, ModuleError> {
        // In dry_run mode, execute step is skipped so output may be absent.
        let Some(output) = ctx.output.as_ref() else {
            return Ok(StepResult::continue_step());
        };
        // INVARIANT: BuiltinModuleLookup runs before this step and sets ctx.module.
        let module = ctx
            .module
            .as_ref()
            .expect("module must be resolved before output_validation");
        let output_schema = module.output_schema();
        validate_against_schema(output, &output_schema, "Output")?;

        // Store redacted output as first-class Context field (symmetric with
        // redacted_inputs). Previously stored under data["_apcore.executor.
        // redacted_output"] which was filtered out by serialize().
        if has_schema(&output_schema) {
            let redacted = redact_sensitive(output, &output_schema);
            ctx.context.redacted_output = Some(
                redacted
                    .as_object()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .collect(),
            );
        }
        ctx.validated_output.clone_from(&ctx.output);
        Ok(StepResult::continue_step())
    }
}

#[async_trait]
impl Step for BuiltinMiddlewareAfter {
    impl_step_meta!(BuiltinMiddlewareAfter);

    async fn execute(&self, ctx: &mut PipelineContext) -> Result<StepResult, ModuleError> {
        let middleware_manager = match ctx.middleware_manager {
            Some(ref mm) => mm.clone(),
            None => return Ok(StepResult::continue_step()),
        };

        // BuiltinExecute normally sets ctx.output before this step, but a custom
        // strategy may reach here with no output (e.g. execute step omitted or
        // skipped). Match Python/TS: pass through instead of panicking.
        let Some(output) = ctx.output.take() else {
            return Ok(StepResult::continue_step());
        };

        match middleware_manager
            .execute_after(&ctx.module_id, ctx.inputs.clone(), output, &ctx.context)
            .await
        {
            Ok(modified_output) => {
                ctx.output = Some(modified_output);
                Ok(StepResult::continue_step())
            }
            Err(e) => {
                // A-D-010 / A-D-012: do NOT run step-level on_error here. The
                // sole on_error recovery site is the executor-level loop
                // (`Executor::call`), which runs `execute_on_error_outcome` over
                // `ctx.executed_middlewares` and honors both Recovery and Retry
                // outcomes. Running it at the step level too made on_error fire
                // twice on an after()-failure (step + executor) and dropped any
                // RetrySignal (step-level `execute_on_error` maps Retry→None).
                // Mirrors apcore-python `BuiltinMiddlewareAfter` (no step-level
                // recovery — it raises) and apcore-typescript. The before-step
                // already populated `ctx.executed_middlewares`, so the executor
                // recovers over exactly the middlewares that ran.
                Err(e)
            }
        }
    }
}

#[async_trait]
impl Step for BuiltinReturnResult {
    impl_step_meta!(BuiltinReturnResult);
    fn requires(&self) -> &[&str] {
        &["output"]
    }

    async fn execute(&self, _ctx: &mut PipelineContext) -> Result<StepResult, ModuleError> {
        // Output is already stored in ctx.output; PipelineEngine returns it.
        Ok(StepResult::continue_step())
    }
}

// ---------------------------------------------------------------------------
// Preset strategies
// ---------------------------------------------------------------------------

/// Build the standard 11-step execution strategy.
///
/// The `module_lookup` step defaults to the process-global toggle store. To
/// bind a per-`APCore` toggle store (issue #71) use
/// [`build_standard_strategy_with_toggle`].
#[must_use]
pub fn build_standard_strategy() -> ExecutionStrategy {
    build_standard_strategy_with_toggle(crate::sys_modules::global_toggle_state_arc())
}

/// Build the standard 11-step execution strategy bound to a specific toggle
/// store for the `module_lookup` read path (issue #71).
///
/// `APCore::with_options` passes its own `Arc<ToggleState>` here so the
/// pipeline observes per-instance toggles rather than the process-global store.
#[must_use]
pub fn build_standard_strategy_with_toggle(
    toggle_state: std::sync::Arc<crate::sys_modules::ToggleState>,
) -> ExecutionStrategy {
    // INVARIANT: all step names are unique literals, so `new()` cannot fail.
    ExecutionStrategy::new(
        "standard",
        vec![
            Box::new(BuiltinContextCreation),
            Box::new(BuiltinCallChainGuard),
            Box::new(BuiltinModuleLookup::new(toggle_state)),
            Box::new(BuiltinACLCheck),
            Box::new(BuiltinApprovalGate),
            Box::new(BuiltinMiddlewareBefore),
            Box::new(BuiltinInputValidation),
            Box::new(BuiltinExecute),
            Box::new(BuiltinOutputValidation),
            Box::new(BuiltinMiddlewareAfter),
            Box::new(BuiltinReturnResult),
        ],
    )
    // INVARIANT: the builtin step list above uses distinct types — duplicates would be a compile-time error.
    .expect("standard strategy should have unique step names")
}

/// Build the internal strategy (standard minus `acl_check` and `approval_gate`).
#[must_use]
pub fn build_internal_strategy() -> ExecutionStrategy {
    let mut s = build_standard_strategy();
    s.set_name("internal");
    s.remove("acl_check").ok();
    s.remove("approval_gate").ok();
    s
}

/// Build the testing strategy (standard minus `acl_check`, `approval_gate`, and `call_chain_guard`).
#[must_use]
pub fn build_testing_strategy() -> ExecutionStrategy {
    let mut s = build_standard_strategy();
    s.set_name("testing");
    s.remove("acl_check").ok();
    s.remove("approval_gate").ok();
    s.remove("call_chain_guard").ok();
    s
}

/// Build the performance strategy (standard minus `middleware_before` and `middleware_after`).
#[must_use]
pub fn build_performance_strategy() -> ExecutionStrategy {
    let mut s = build_standard_strategy();
    s.set_name("performance");
    s.remove("middleware_before").ok();
    s.remove("middleware_after").ok();
    s
}

/// Build a minimal strategy: context → lookup → execute → return only.
///
/// No safety checks, no ACL, no approval, no validation, no middleware.
/// Suitable for pre-validated internal hot paths. Use with caution.
#[must_use]
pub fn build_minimal_strategy() -> ExecutionStrategy {
    let mut s = build_standard_strategy();
    s.set_name("minimal");
    s.remove("call_chain_guard").ok();
    s.remove("acl_check").ok();
    s.remove("approval_gate").ok();
    s.remove("middleware_before").ok();
    s.remove("input_validation").ok();
    s.remove("output_validation").ok();
    s.remove("middleware_after").ok();
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::Context;
    use crate::context::Identity;

    fn empty_context() -> PipelineContext {
        let identity = Identity::new(
            "user-1".to_string(),
            "user".to_string(),
            vec![],
            HashMap::new(),
        );
        let context: Context<serde_json::Value> = Context::new(identity);
        PipelineContext::new("mod-x", serde_json::json!({}), context, "custom")
    }

    // [exec-none-output-panic] A custom strategy may reach the after-steps with
    // ctx.output == None. Python/TS pass through; Rust must not panic.
    #[tokio::test]
    async fn output_validation_passes_through_when_output_none() {
        let step = BuiltinOutputValidation;
        let mut ctx = empty_context();
        assert!(ctx.output.is_none());
        let result = step.execute(&mut ctx).await;
        assert!(result.is_ok(), "must not panic/err when output is None");
    }

    #[tokio::test]
    async fn middleware_after_passes_through_when_output_none() {
        use crate::middleware::manager::MiddlewareManager;
        use std::sync::Arc;

        let step = BuiltinMiddlewareAfter;
        let mut ctx = empty_context();
        // Attach a manager so the step reaches the output `take()` path; with
        // output None it must pass through (continue) rather than panic.
        ctx.middleware_manager = Some(Arc::new(MiddlewareManager::new()));
        assert!(ctx.output.is_none());
        let result = step.execute(&mut ctx).await;
        assert!(result.is_ok(), "must not panic/err when output is None");
    }
}

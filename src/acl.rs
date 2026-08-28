// APCore Protocol — Access Control Lists
// Spec reference: ACL rules, audit entries

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_yaml_ng as serde_yaml;
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Once};

use crate::acl_handlers::{
    evaluate_conditions_async_outcome as handlers_evaluate_conditions_async_outcome,
    precheck_conditions, register_builtin_handlers, ConditionOutcome, PrecheckPath, RuleFault,
    CONDITION_HANDLERS,
};
use crate::context::Context;
use crate::errors::{ErrorCode, ModuleError};
use crate::utils::match_pattern;

/// Defines an access control rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ACLRule {
    #[serde(default)]
    pub callers: Vec<String>,
    #[serde(default)]
    pub targets: Vec<String>,
    pub effect: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conditions: Option<serde_json::Value>,
}

/// Audit log entry produced by ACL checks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub timestamp: String,
    pub caller_id: String,
    pub target_id: String,
    pub decision: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_rule: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_rule_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity_type: Option<String>,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_depth: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    /// Error message from a condition handler that panicked or returned an error
    /// during evaluation, if any. Cross-language parity with apcore-python
    /// AuditEntry.handler_error and apcore-typescript AuditEntry.handlerError
    /// (sync finding A-D-024).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handler_error: Option<String>,
}

/// Type alias for the audit logger callback.
type AuditLoggerFn = dyn Fn(&AuditEntry) + Send + Sync;

/// One rule that fails PROTOCOL_SPEC §6.1.4's structural and registry
/// precheck.
///
/// Produced by [`ACL::validate_rules`] (§6.1.2 rule 3, §6.1.3). A fault is
/// either a condition key with no resolvable handler, a compound operator
/// whose operand has the wrong shape (`$or` that is not a list, `$not` that is
/// not an object), or a `conditions` value that is not a mapping at all.
///
/// `sync_resolvable` and `async_resolvable` are reported separately and MUST
/// NOT be collapsed into a single boolean: `async_check()` consults the async
/// registry and falls back to the sync one, while `check()` consults only the
/// sync registry, so a key registered **only** as an async handler is a working
/// condition on one path and an unevaluable one on the other. They are named
/// `*_resolvable` rather than `*_registered` because they mean "resolvable on
/// that evaluation path", not "present in that registry" — `async_resolvable`
/// is the union of both, and would otherwise read as false for every built-in
/// leaf handler (§6.1.3 rule 2).
///
/// Marked `#[non_exhaustive]` so a future spec revision can add a field without
/// a major version bump. That works by removing struct-literal construction
/// from every crate but this one — `..Default::default()` included, since it is
/// itself a struct expression (`error[E0639]`). Build one with
/// [`RuleValidationFinding::new`]; the fields are public to read.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RuleValidationFinding {
    /// Index of the offending rule in definition order.
    pub rule_index: usize,
    /// Where the fault sits in the rule (§6.1.4 condition paths): `roles`,
    /// `$or[1].mispelled`, `$not.k`, or `$` for a non-mapping `conditions`.
    ///
    /// Findings order by this, not by key, because a nested `$or` may carry the
    /// same key at several positions and ordering by key alone is undefined.
    pub condition_path: String,
    /// The offending key, where one exists. `None` for a `conditions` value
    /// that is not a mapping — there is no key to name.
    pub condition_key: Option<String>,
    /// The rule's effect. A finding on a `deny` rule is the consequential one:
    /// per §6.1.1 that rule now denies every call it matches.
    pub effect: String,
    /// Whether the condition resolves for `check()`.
    pub sync_resolvable: bool,
    /// Whether it resolves for `async_check()`.
    pub async_resolvable: bool,
}

impl RuleValidationFinding {
    /// Build a finding. Provided because `#[non_exhaustive]` removes
    /// struct-literal construction for downstream crates
    /// (`api-surface-conventions.md` §9.2 rule 2).
    #[must_use]
    pub fn new(
        rule_index: usize,
        condition_path: impl Into<String>,
        condition_key: Option<String>,
        effect: impl Into<String>,
        sync_resolvable: bool,
        async_resolvable: bool,
    ) -> Self {
        Self {
            rule_index,
            condition_path: condition_path.into(),
            condition_key,
            effect: effect.into(),
            sync_resolvable,
            async_resolvable,
        }
    }
}

/// Outcome of matching one ACL rule against a call (PROTOCOL_SPEC §6.3).
///
/// Three variants rather than a `bool`, because a rule whose conditions could
/// not be evaluated is neither "matched" nor "did not match" until the rule's
/// `effect` is consulted (§6.1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuleMatch {
    /// Patterns did not match, or the conditions were UNSATISFIED.
    NoMatch,
    /// Patterns matched and the conditions (if any) were SATISFIED.
    Match,
    /// Patterns matched but the conditions were UNEVALUABLE (§6.1.1).
    Unevaluable,
}

/// Access control list manager.
///
/// Thread safety: Rust's borrow checker enforces exclusive access for mutation
/// (&mut self for `add_rule/remove_rule/reload`). The `check()` method takes &self
/// and is safe for concurrent reads. No internal lock is needed.
pub struct ACL {
    rules: Vec<ACLRule>,
    default_effect: String,
    yaml_path: Option<String>,
    audit_logger: Option<Arc<AuditLoggerFn>>,
}

impl std::fmt::Debug for ACL {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ACL")
            .field("rules", &self.rules)
            .field("default_effect", &self.default_effect)
            .field("yaml_path", &self.yaml_path)
            .field("audit_logger", &self.audit_logger.as_ref().map(|_| "..."))
            .finish()
    }
}

impl Clone for ACL {
    fn clone(&self) -> Self {
        Self {
            rules: self.rules.clone(),
            default_effect: self.default_effect.clone(),
            yaml_path: self.yaml_path.clone(),
            audit_logger: self.audit_logger.clone(),
        }
    }
}

impl ACL {
    /// Create a new ACL with the given rules, default effect, and optional audit logger.
    ///
    /// # Errors
    ///
    /// Returns `ModuleError` with `ErrorCode::ACLRuleError` if `default_effect` is
    /// not `"allow"` or `"deny"`, matching the constructor validation in
    /// apcore-typescript (sync finding A-D-025).
    pub fn try_new(
        rules: Vec<ACLRule>,
        default_effect: impl Into<String>,
        audit_logger: Option<Arc<AuditLoggerFn>>,
    ) -> Result<Self, ModuleError> {
        let default_effect = default_effect.into();
        if default_effect != "allow" && default_effect != "deny" {
            return Err(ModuleError::new(
                ErrorCode::ACLRuleError,
                format!("Invalid default_effect '{default_effect}': must be 'allow' or 'deny'"),
            ));
        }
        // Validate each rule's effect at load, matching apcore-python
        // `acl.py` and apcore-typescript `acl.ts` (both raise ACLRuleError
        // on a rule effect that is not 'allow' or 'deny').
        for (index, rule) in rules.iter().enumerate() {
            if rule.effect != "allow" && rule.effect != "deny" {
                return Err(ModuleError::new(
                    ErrorCode::ACLRuleError,
                    format!(
                        "Rule {index} has invalid effect '{}', must be 'allow' or 'deny'",
                        rule.effect
                    ),
                ));
            }
        }
        Ok(Self::new_unchecked(rules, default_effect, audit_logger))
    }

    /// Create a new ACL with the given rules, default effect, and optional
    /// audit logger.
    ///
    /// **Validates** `default_effect` and panics on invalid input. This
    /// matches apcore-python and apcore-typescript constructor behaviour
    /// (both throw on invalid `default_effect`) — sync finding A-D-302.
    ///
    /// For fallible construction (e.g., when `default_effect` originates
    /// from user input or a YAML file), prefer [`ACL::try_new`].
    ///
    /// # Panics
    ///
    /// Panics if `default_effect` is not `"allow"` or `"deny"`.
    pub fn new(
        rules: Vec<ACLRule>,
        default_effect: impl Into<String>,
        audit_logger: Option<Arc<AuditLoggerFn>>,
    ) -> Self {
        Self::try_new(rules, default_effect, audit_logger)
            .expect("invalid default_effect — use ACL::try_new for fallible construction")
    }

    fn new_unchecked(
        rules: Vec<ACLRule>,
        default_effect: impl Into<String>,
        audit_logger: Option<Arc<AuditLoggerFn>>,
    ) -> Self {
        // Auto-register built-in condition handlers ($or, $not, identity_types,
        // roles, max_call_depth) — matches apcore-python and apcore-typescript
        // module-load auto-registration. Idempotent via std::sync::Once
        // (sync finding A-D-027), and each built-in is seeded only where the
        // key is unclaimed, so a handler the deployment registered before the
        // first ACL is not replaced by the permissive default (A-D-010).
        Self::init_builtin_handlers();
        // PROTOCOL_SPEC §6.1.2: warn (never fail) for every rule that fails
        // §6.1.4's precheck on the sync path. Emitted AFTER
        // `init_builtin_handlers` so the built-ins are never reported. Rule 4
        // makes direct construction an entry point that MUST be covered, not
        // just file loading — `try_new`, `new`, `load`, `discover` and
        // `reload` all funnel through here.
        Self::warn_rule_faults(&rules, 0);
        Self {
            rules,
            default_effect: default_effect.into(),
            yaml_path: None,
            audit_logger,
        }
    }

    /// Emit PROTOCOL_SPEC §6.1.2 rule 2's load-time warning for every rule that
    /// fails §6.1.4's precheck on the sync path.
    ///
    /// `index_offset` is added to the position within `rules`, so `add_rule`
    /// can report the index the rule will actually occupy.
    ///
    /// Loading MUST NOT fail here: `register_condition()` writes to a runtime,
    /// process-wide registry, and `acl.root` discovery commonly runs during
    /// framework bootstrap ahead of application code, so failing would reject
    /// valid configurations on ordering alone. [`ACL::validate_rules`] is the
    /// deterministic check to run once registration is complete.
    fn warn_rule_faults(rules: &[ACLRule], index_offset: usize) {
        for (i, rule) in rules.iter().enumerate() {
            let Some(conditions) = rule.conditions.as_ref() else {
                continue;
            };
            for fault in precheck_conditions(conditions, PrecheckPath::Sync) {
                tracing::warn!(
                    rule_index = index_offset + i,
                    condition_path = %fault.path,
                    condition_key = fault.key.as_deref().unwrap_or("-"),
                    effect = %rule.effect,
                    async_resolvable = fault.async_resolvable,
                    reason = %fault.reason,
                    "ACL rule fails the §6.1.4 precheck; it will be UNEVALUABLE \
                     (PROTOCOL_SPEC §6.1.1) — a deny rule denies, an allow rule does not \
                     grant. Fix the rule or register a handler, and call \
                     ACL::validate_rules() after bootstrap to assert on this."
                );
            }
        }
    }

    /// Set the audit logger callback.
    pub fn set_audit_logger(&mut self, logger: impl Fn(&AuditEntry) + Send + Sync + 'static) {
        self.audit_logger = Some(Arc::new(logger));
    }

    /// Evaluate all conditions with three-valued AND logic using the sync
    /// handler registry (PROTOCOL_SPEC §6.1.1).
    ///
    /// Returns a [`ConditionOutcome`], not a `bool`: "a handler answered no"
    /// and "no answer was obtainable" reach the rule loop as different values,
    /// because a `deny` rule resolves them in opposite directions. §6.3 makes
    /// carrying that distinction a MUST — collapsing the two is what let a
    /// misspelled condition key silently disable a `deny` rule before v1.22.0.
    ///
    /// This is a **sync** function. It drives each handler's future by polling
    /// it once with a noop waker:
    ///
    /// * `Poll::Ready(v)` — the handler's answer, `Satisfied` / `Unsatisfied`.
    /// * `Poll::Pending` — §6.1.1 case 3: the handler is genuinely async
    ///   and could not be resolved on this path, so the condition is
    ///   **`Unevaluable`**, not unsatisfied. Callers with I/O-performing
    ///   handlers must use [`ACL::async_check`].
    /// * a panic — §6.1.1 case 2, caught here and reported `Unevaluable`.
    ///
    /// A key with no handler in [`CONDITION_HANDLERS`] is §6.1.1 case 1.
    /// Note that the async registry is deliberately NOT consulted: §6.1.3 makes
    /// an async-only key unevaluable on the sync path by design.
    ///
    /// **Every** child is evaluated — no short-circuit. §6.1.1 permits
    /// short-circuiting AND on the first `Unsatisfied` and explicitly allows
    /// evaluating every child instead "for deterministic diagnostics"; the
    /// latter is what makes `handler_error` list every unevaluable key (rule 2)
    /// rather than whichever one map iteration happened to reach first. The
    /// decision is identical either way.
    ///
    /// **Architecture note:** two parallel paths exist — this sync path and the
    /// async [`Self::evaluate_conditions_async_outcome`]. Keep both in sync when
    /// adding new condition logic to avoid drift. New conditions should be
    /// tested against both paths.
    pub fn evaluate_conditions(
        conditions: &HashMap<String, serde_json::Value>,
        ctx: &Context<serde_json::Value>,
    ) -> ConditionOutcome {
        // Resolve handlers first; an unresolved key is a leaf outcome of its own.
        let mut to_evaluate = Vec::with_capacity(conditions.len());
        {
            let handlers = CONDITION_HANDLERS.read();
            for (key, value) in conditions {
                to_evaluate.push((key, handlers.get(key.as_str()).cloned(), value));
            }
        }

        // AND over three-valued children, starting from the vacuous truth of an
        // empty `conditions` object (which keeps `$not: {}` fail-closed).
        let mut outcome = ConditionOutcome::Satisfied;
        for (key, handler, value) in to_evaluate {
            let Some(handler) = handler else {
                // §6.1.1 case 1: no resolvable handler.
                tracing::warn!(
                    condition = %key,
                    "Unknown ACL condition — unevaluable (PROTOCOL_SPEC §6.1.1)"
                );
                crate::acl_handlers::report_condition_unevaluable(key, "unknown ACL condition");
                outcome = outcome.and(ConditionOutcome::Unevaluable);
                continue;
            };

            // A-D-011 (SECURITY): wrap both the future construction and the
            // single poll in catch_unwind so a panicking custom handler does
            // NOT unwind out of the ACL gate. Mirrors the Python `try/except`
            // and TypeScript `try/catch` around handler.evaluate.
            let poll_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let fut = handler.evaluate_outcome(value, ctx);
                let mut fut = std::pin::pin!(fut);
                let waker = std::task::Waker::noop();
                let mut cx = std::task::Context::from_waker(waker);
                fut.as_mut().poll(&mut cx)
            }));
            let child = match poll_result {
                Ok(std::task::Poll::Ready(val)) => val,
                Ok(std::task::Poll::Pending) => {
                    // §6.1.1 case 3. Through spec v1.21.0 this was
                    // "treated as unsatisfied", which made a `deny` rule
                    // guarded by an async-only handler inert on the sync path
                    // — the same failure mode as a misspelled key, reached by
                    // a different route.
                    tracing::warn!(
                        condition = %key,
                        "Async ACL condition not ready in a sync context — unevaluable \
                         (PROTOCOL_SPEC §6.1.1; use ACL::async_check)"
                    );
                    crate::acl_handlers::report_condition_unevaluable(
                        key,
                        "handler is not ready synchronously (use ACL::async_check)",
                    );
                    ConditionOutcome::Unevaluable
                }
                Err(payload) => {
                    let msg = crate::acl_handlers::panic_message(payload.as_ref());
                    tracing::error!(
                        condition = %key,
                        panic = %msg,
                        "ACL condition handler panicked — unevaluable (PROTOCOL_SPEC §6.1.1)"
                    );
                    crate::acl_handlers::report_condition_unevaluable(
                        key,
                        format!("handler panicked: {msg}"),
                    );
                    ConditionOutcome::Unevaluable
                }
            };
            outcome = outcome.and(child);
        }
        outcome
    }

    /// Async evaluate all conditions with AND logic using the handler registry.
    ///
    /// Boolean façade over [`Self::evaluate_conditions_async_outcome`]. An
    /// unevaluable result maps to `false`, which is exactly the collapse
    /// §6.1.1 forbids the **rule loop** from making — use the `_outcome` form
    /// anywhere the rule's `effect` matters.
    pub async fn evaluate_conditions_async(
        conditions: &HashMap<String, serde_json::Value>,
        ctx: &Context<serde_json::Value>,
    ) -> bool {
        Self::evaluate_conditions_async_outcome(conditions, ctx)
            .await
            .is_satisfied()
    }

    /// Async, three-valued counterpart to [`Self::evaluate_conditions`]
    /// (PROTOCOL_SPEC §6.1.1).
    ///
    /// Delegates to `acl_handlers::evaluate_conditions_async_outcome` so
    /// compound operators (`$or`, `$not`) share the same async evaluation path.
    pub async fn evaluate_conditions_async_outcome(
        conditions: &HashMap<String, serde_json::Value>,
        ctx: &Context<serde_json::Value>,
    ) -> ConditionOutcome {
        handlers_evaluate_conditions_async_outcome(conditions, ctx).await
    }

    /// Add a rule to the ACL (inserted at position 0, highest priority).
    ///
    /// Spec contract `acl-system.md` §Contract.ACL.add_rule declares
    /// `On success: None` — the body is infallible, so no `Result` wrapper.
    ///
    /// PROTOCOL_SPEC §6.1.2 rule 4 makes runtime rule insertion an entry point
    /// that MUST be covered by the load-time condition-key check: if the rule
    /// carries `conditions`, every key it references — including keys nested
    /// inside `$or` / `$not` — is checked against the sync handler registry and
    /// a warning naming the rule index (`0`), the key and the rule's `effect`
    /// is emitted for each that does not resolve. Insertion still succeeds;
    /// this is the same warn-never-fail contract [`ACL::load`] has, for the
    /// same reason.
    pub fn add_rule(&mut self, rule: ACLRule) {
        Self::warn_rule_faults(std::slice::from_ref(&rule), 0);
        self.rules.insert(0, rule);
    }

    /// Report every rule that fails PROTOCOL_SPEC §6.1.4's precheck
    /// (§6.1.2 rule 3, §6.1.3).
    ///
    /// Named `validate_rules` rather than `validate_conditions` because it
    /// reports structural faults in the rule as a whole, not only unresolvable
    /// condition keys: a `$or` whose value is not a list, a `$not` whose value
    /// is not an object, and a `conditions` that is not a mapping are all
    /// findings. (§6.1.4.1's malformed `callers` / `targets` cannot arise here
    /// — see the note below.)
    ///
    /// Condition handlers are registered at runtime into a process-wide
    /// registry, and an ACL may legitimately be loaded before a deployment
    /// registers its custom handlers, so loading only warns. This method is the
    /// deterministic check to run **after** registration is complete: the
    /// intended shape is to call it once bootstrap has finished and to treat
    /// any finding on a `deny` rule as a startup error.
    ///
    /// A finding is emitted whenever `sync_resolvable` is false — **including**
    /// when `async_resolvable` is true. §6.1.3: `check()` consults only the
    /// sync registry, so an async-only key is a working condition under
    /// `async_check()` and an unevaluable one under `check()`. A caller that
    /// only ever uses `async_check()` may ignore such a finding; that choice
    /// belongs to the caller, not to the validator.
    ///
    /// Pure read — never mutates the ACL, never registers a handler, never
    /// emits an audit event. Findings are ordered by rule index, then
    /// ascending by condition **path**. An empty result is not a guarantee
    /// about the future: a later [`Self::add_rule`] can introduce a new fault.
    ///
    /// This is diagnostics, not enforcement. The guarantee that a broken `deny`
    /// rule cannot silently pass traffic is §6.1.1's, and holds whether or not
    /// anyone calls this.
    ///
    /// # `callers` / `targets` are checked by the type system
    ///
    /// §6.1.4.1 requires the precheck to classify a non-list `callers` /
    /// `targets` as unevaluable, because a bare string is iterable in several
    /// host languages: `callers: "admin.*"` iterates character by character,
    /// and the `*` character matches everything, so the typo grants access to
    /// every caller. [`ACLRule::callers`] and [`ACLRule::targets`] are
    /// `Vec<String>`, so the malformed value is unrepresentable — serde rejects
    /// a bare string at deserialization and the compiler rejects one in a
    /// struct literal. §6.1.4.1 states explicitly that such an implementation
    /// "satisfies this clause by construction and needs no runtime check", so
    /// there is deliberately no check here for a state that cannot exist.
    #[must_use]
    pub fn validate_rules(&self) -> Vec<RuleValidationFinding> {
        let mut findings = Vec::new();
        for (idx, rule) in self.rules.iter().enumerate() {
            let Some(conditions) = rule.conditions.as_ref() else {
                continue;
            };
            // `precheck_conditions` already returns faults ordered by path, and
            // rules are walked in definition order, so the result is ordered by
            // (rule_index, condition_path) as §6.1.2 rule 3 requires.
            for fault in precheck_conditions(conditions, PrecheckPath::Sync) {
                findings.push(RuleValidationFinding::new(
                    idx,
                    fault.path,
                    fault.key,
                    rule.effect.clone(),
                    fault.sync_resolvable,
                    fault.async_resolvable,
                ));
            }
        }
        findings
    }

    /// Remove the first rule matching the given callers and targets.
    /// Returns true if a rule was removed.
    pub fn remove_rule(&mut self, callers: &[String], targets: &[String]) -> bool {
        self.remove_rule_with_conditions(callers, targets, None)
    }

    /// Remove the first rule matching callers, targets, and (optional) conditions.
    ///
    /// When `conditions` is `Some(value)`, additionally disambiguate by
    /// `ACLRule.conditions` via JSON value equality. Two rules with identical
    /// callers+targets but different conditions can be selectively removed by
    /// passing the matching conditions. Cross-language parity with
    /// apcore-typescript removeRule (sync finding A-D-026).
    pub fn remove_rule_with_conditions(
        &mut self,
        callers: &[String],
        targets: &[String],
        conditions: Option<&serde_json::Value>,
    ) -> bool {
        if let Some(pos) = self.rules.iter().position(|r| {
            if r.callers != callers || r.targets != targets {
                return false;
            }
            match conditions {
                Some(want) => r.conditions.as_ref() == Some(want),
                None => true,
            }
        }) {
            self.rules.remove(pos);
            true
        } else {
            false
        }
    }

    /// Check whether the given caller is allowed to access the target.
    /// Uses first-match-wins evaluation. Maps `None` caller to `@external`.
    ///
    /// Returns `true` for allow, `false` for deny. Never errors — deny is
    /// signalled via the return value, not an `Err`, per the protocol spec.
    ///
    /// Sync entry point. The shared post-decision audit logic lives in
    /// `finalize_*` helpers so this method and `async_check` cannot drift.
    pub fn check(
        &self,
        caller_id: Option<&str>,
        target_id: &str,
        ctx: Option<&Context<serde_json::Value>>,
    ) -> bool {
        // Wrap the entire evaluation in a synchronous handler-error capture
        // scope (A-D-002) so any condition handler that calls
        // `report_handler_error(...)` lands its message in this call's audit
        // entry — mirroring `async_check`'s task-local scope and Python's
        // sync `_handler_error_var.set(None)` / `reset(token)` pairing.
        // Without this, `build_audit_entry` always read `None` on the sync
        // path because the only scope was tokio-task-local.
        let (decision, _captured) = crate::acl_handlers::with_handler_error_capture_sync(|| {
            self.check_inner(caller_id, target_id, ctx)
        });
        decision
    }

    fn check_inner(
        &self,
        caller_id: Option<&str>,
        target_id: &str,
        ctx: Option<&Context<serde_json::Value>>,
    ) -> bool {
        let caller = caller_id.unwrap_or("@external");

        // Snapshot rules + default_effect before evaluation so any concurrent
        // add_rule/reload caller (wrapped in Arc<RwLock<ACL>> by the user) does
        // not mutate the list mid-check. Matches apcore-python's
        // _snapshot() (acl.py:282) and apcore-typescript's rules.slice()
        // (acl.ts:203) — sync finding A-D-021.
        let rules: Vec<ACLRule> = self.rules.clone();
        let default_effect = self.default_effect.clone();

        if rules.is_empty() {
            return self.finalize_no_rules(&default_effect, caller, target_id, ctx);
        }

        for (idx, rule) in rules.iter().enumerate() {
            let paths_before = crate::acl_handlers::reported_condition_paths();
            match self.matches_rule(rule, caller, target_id, ctx) {
                RuleMatch::Match => {
                    return self.finalize_rule_match(idx, rule, caller, target_id, ctx)
                }
                RuleMatch::Unevaluable => {
                    if Self::resolve_unevaluable_rule(idx, rule, &paths_before) {
                        return self.finalize_rule_match(idx, rule, caller, target_id, ctx);
                    }
                }
                RuleMatch::NoMatch => {}
            }
        }

        // Use the snapshotted default_effect rather than re-reading
        // self.default_effect to maintain consistency with the snapshotted
        // rules (sync finding A-D-021 / A-D-301).
        self.finalize_default_effect(&default_effect, caller, target_id, ctx)
    }

    /// Load ACL rules from a YAML file.
    pub fn load(path: &str) -> Result<Self, ModuleError> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            ModuleError::new(
                ErrorCode::ConfigNotFound,
                format!("Failed to read ACL file '{path}': {e}"),
            )
        })?;

        // Sync finding A-D-022: structural ACL parse/validation errors carry
        // `ErrorCode::ACLRuleError` per spec contract — apcore-python and
        // apcore-typescript both raise `ACLRuleError`. Previously Rust used
        // `ErrorCode::ConfigInvalid`, which broke cross-language fixtures
        // asserting on the error code.
        let raw: serde_json::Value = serde_yaml::from_str(&content).map_err(|e| {
            ModuleError::new(
                ErrorCode::ACLRuleError,
                format!("Failed to parse ACL file '{path}': {e}"),
            )
        })?;

        // Expect top-level "rules" key.
        let rules_val = raw.get("rules").ok_or_else(|| {
            ModuleError::new(
                ErrorCode::ACLRuleError,
                format!("ACL file '{path}' missing 'rules' key"),
            )
        })?;

        // A12-ACL: every rule MUST explicitly declare `callers` and `targets`.
        // `#[serde(default)]` on those fields would otherwise let an omitted
        // key load as an empty Vec, silently turning a deny rule inert. Reject
        // a missing key here (the key may be an empty list — only OMISSION is
        // rejected), matching apcore-python acl.py:270 and apcore-typescript
        // acl.ts:74 which raise ACLRuleError at load (sync finding A-D-09).
        if let Some(rules_arr) = rules_val.as_array() {
            for (i, raw_rule) in rules_arr.iter().enumerate() {
                let Some(obj) = raw_rule.as_object() else {
                    return Err(ModuleError::new(
                        ErrorCode::ACLRuleError,
                        format!("ACL rule {i} in '{path}' must be a mapping"),
                    ));
                };
                for key in ["callers", "targets"] {
                    if !obj.contains_key(key) {
                        return Err(ModuleError::new(
                            ErrorCode::ACLRuleError,
                            format!("ACL rule {i} in '{path}' missing required key '{key}'"),
                        ));
                    }
                }
            }
        }

        let rules: Vec<ACLRule> = serde_json::from_value(rules_val.clone()).map_err(|e| {
            ModuleError::new(
                ErrorCode::ACLRuleError,
                format!("Invalid ACL rules in '{path}': {e}"),
            )
        })?;

        // Distinguish "key absent" (→ canonical default `deny`) from "key
        // present but not a string". The previous
        // `.and_then(as_str).unwrap_or("deny")` silently coerced a non-string
        // such as `default_effect: true` into a valid value, so `try_new`'s
        // validation never fired and an operator typo went unreported.
        // apcore-python and apcore-typescript both raise `ACLRuleError` here.
        // The outcome was fail-closed either way, so this is a diagnostics fix,
        // not a bypass.
        let default_effect = match raw.get("default_effect") {
            None | Some(serde_json::Value::Null) => "deny".to_string(),
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(other) => {
                return Err(ModuleError::new(
                    ErrorCode::ACLRuleError,
                    format!(
                        "ACL file '{path}': default_effect must be the string 'allow' or 'deny' (got {other})"
                    ),
                ))
            }
        };

        // Propagate try_new validation errors as Result rather than panicking
        // — YAML errors must not crash the host process (sync finding A-D-302).
        let mut acl = Self::try_new(rules, default_effect, None).map_err(|e| {
            ModuleError::new(
                ErrorCode::ACLRuleError,
                format!("Invalid ACL config in '{path}': {}", e.message),
            )
        })?;
        acl.yaml_path = Some(path.to_string());
        Ok(acl)
    }

    /// Activate `acl.root` config-driven ACL discovery (D-64, Recommendation A).
    ///
    /// Resolves the `acl.root` config key (default `"./acl"`) and loads an ACL
    /// from it when the path exists. The path is resolved relative to the
    /// directory of the config's source file when known
    /// ([`Config::source_path`]), otherwise relative to the current working
    /// directory.
    ///
    /// `acl.root` is a directory by spec convention (`acl/{scope}_acl.yaml`,
    /// PROTOCOL_SPEC §3.1). When the resolved path is a directory, the
    /// conventional `global_acl.yaml` within it is loaded; if that file is
    /// absent the result is `None` (the missing-path no-op still holds). When
    /// the resolved path is itself a file, it is loaded directly. Either way the
    /// actual load goes through [`ACL::load`].
    ///
    /// **Critical invariant:** a missing path returns `None` and attaches
    /// NOTHING. It MUST NOT synthesize an empty default-deny ACL — doing so
    /// would silently deny every inter-module call in every project that lacks
    /// an `acl` dir. Missing path means "no enforcement", identical to pre-D-64
    /// behavior. The `acl.default_effect` config key only takes effect once a
    /// real ACL file is loaded (it is read by [`ACL::load`] from the ACL file
    /// itself); it never feeds a synthesized ACL here.
    ///
    /// # Errors
    /// Returns [`ModuleError`] only when a resolved ACL file exists but is
    /// structurally invalid (propagated from [`ACL::load`] with
    /// `ErrorCode::ACLRuleError`). A missing root is never an error — it yields
    /// `Ok(None)`.
    pub fn discover(config: &crate::config::Config) -> Result<Option<Self>, ModuleError> {
        let Some(root) = config
            .get("acl.root")
            .and_then(|v| v.as_str().map(std::string::ToString::to_string))
        else {
            return Ok(None);
        };

        let mut root_path = std::path::PathBuf::from(&root);
        if !root_path.is_absolute() {
            let base = match config.source_path() {
                Some(source) => source
                    .canonicalize()
                    .unwrap_or_else(|_| source.to_path_buf())
                    .parent()
                    .map_or_else(
                        || std::path::PathBuf::from("."),
                        std::path::Path::to_path_buf,
                    ),
                None => std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
            };
            root_path = base.join(&root_path);
        }

        if !root_path.exists() {
            // Missing path -> no enforcement. Do NOT synthesize an ACL.
            return Ok(None);
        }

        if root_path.is_dir() {
            // Directory convention: acl/{scope}_acl.yaml (PROTOCOL_SPEC §3.1).
            let acl_file = root_path.join("global_acl.yaml");
            if !acl_file.is_file() {
                // Directory present but no conventional ACL file -> no-op.
                return Ok(None);
            }
            return Self::load(&acl_file.to_string_lossy()).map(Some);
        }

        Self::load(&root_path.to_string_lossy()).map(Some)
    }

    /// Register a custom condition handler. Delegates to `acl_handlers::register_condition`.
    pub fn register_condition(
        key: impl Into<String>,
        handler: std::sync::Arc<dyn crate::acl_handlers::ACLConditionHandler>,
    ) {
        crate::acl_handlers::register_condition(key, handler);
    }

    /// Register a custom async-only condition handler.
    ///
    /// Stored in a **separate registry** from sync handlers — `async_check`
    /// consults the async registry first, then falls back to the sync
    /// registry. This lets callers override a sync handler for a given key
    /// with an async-only variant without affecting the sync `ACL::check`
    /// path. Cross-language parity with apcore-python
    /// `register_async_condition` and apcore-typescript
    /// `registerAsyncCondition` (closes A-D-ACL-002).
    pub fn register_async_condition(
        key: impl Into<String>,
        handler: std::sync::Arc<dyn crate::acl_handlers::ACLConditionHandler>,
    ) {
        crate::acl_handlers::register_async_condition(key, handler);
    }

    /// Reload rules from the stored YAML path.
    ///
    /// **Deadlock avoidance:** the borrow on `self.yaml_path` is released
    /// (via clone + scope) *before* the blocking file I/O in [`Self::load`]
    /// begins. This matters when the caller holds the ACL inside an
    /// `Arc<RwLock<ACL>>`-style wrapper and an audit logger or condition
    /// handler tries to read the same lock from another thread mid-reload.
    /// Holding `&mut self` across `Self::load` would block any concurrent
    /// reader for the duration of the file read; the brace scope below
    /// makes that explicitly impossible (sync finding A-D-303).
    pub fn reload(&mut self) -> Result<(), ModuleError> {
        let path = {
            // Narrow scope: the immutable borrow on `self.yaml_path` ends
            // at the closing brace, *before* file I/O is initiated below.
            self.yaml_path.clone().ok_or_else(|| {
                ModuleError::new(
                    ErrorCode::ACLRuleError,
                    "Cannot reload: ACL was not loaded from a YAML file".to_string(),
                )
            })?
        };

        // File I/O happens here with no outstanding borrow on `self`.
        let reloaded = Self::load(&path)?;

        self.rules = reloaded.rules;
        self.default_effect = reloaded.default_effect;
        // `self.yaml_path` is intentionally left untouched: reload re-reads the
        // *stored* path, so reassigning it is a no-op (reloaded.yaml_path always
        // equals the existing path). Matches apcore-python / apcore-typescript,
        // which do not re-set the path on reload.
        Ok(())
    }

    /// The current rule list, in definition order (PROTOCOL_SPEC §6.8).
    ///
    /// Read-only introspection: an immutable slice, so a caller can neither
    /// reorder nor mutate the ACL's own list through it (§6.8 rule 3). It is a
    /// pure read — no audit event, no state change, no lock the caller has to
    /// release — and it reads the live object, so it reflects a
    /// [`Self::reload`] (§6.8 rule 4).
    #[must_use]
    pub fn rules(&self) -> &[ACLRule] {
        &self.rules
    }

    /// The effect applied when no rule matches — `"allow"` or `"deny"`
    /// (PROTOCOL_SPEC §6.8).
    ///
    /// `default_effect` is the single most consequential value in an ACL, and
    /// §6.8 makes reading it back a MUST: without it, tooling that reports or
    /// audits the enforced policy has to re-read and re-parse the ACL file to
    /// recover a value the loaded object already holds — a second copy that can
    /// drift across [`Self::reload`], and on Rust the only option available at
    /// all while the field was private.
    ///
    /// Pure read, and reads the live object, so it reflects a `reload()`.
    #[must_use]
    pub fn default_effect(&self) -> &str {
        &self.default_effect
    }

    // --- Private helpers ---

    /// Apply PROTOCOL_SPEC §6.1.1 to a rule whose conditions were UNEVALUABLE.
    ///
    /// Returns `true` when the rule must take effect (so the caller finalizes
    /// it as a rule match — a `deny` rule denies), `false` when evaluation must
    /// continue with the next rule (an `allow` rule MUST NOT grant).
    ///
    /// Also emits §6.1.1 rule 3's warning, which must name the condition
    /// **path**, the rule's index and the rule's `effect`. The `effect` is in
    /// the message because a misconfigured `deny` rule is the consequential
    /// case. The paths are recovered by diffing the per-call handler-error
    /// scope, which is where both precheck-origin and execution-origin faults
    /// land — including one nested inside `$or` / `$not`.
    fn resolve_unevaluable_rule(idx: usize, rule: &ACLRule, paths_before: &[String]) -> bool {
        let paths_after = crate::acl_handlers::reported_condition_paths();
        let new_paths: Vec<String> = paths_after
            .into_iter()
            .filter(|p| !paths_before.contains(p))
            .collect();
        let conditions = if new_paths.is_empty() {
            // Already reported by an earlier rule in this same check().
            "(see AuditEntry.handler_error)".to_string()
        } else {
            new_paths.join(", ")
        };

        let takes_effect = rule.effect == "deny";
        tracing::warn!(
            rule_index = idx,
            effect = %rule.effect,
            condition_paths = %conditions,
            "ACL rule has unevaluable conditions — {} (PROTOCOL_SPEC §6.1.1)",
            if takes_effect {
                "the deny rule TAKES EFFECT and the call is denied"
            } else {
                "the allow rule does not match and MUST NOT grant"
            }
        );
        takes_effect
    }

    /// Check if a rule matches the caller, target, and context.
    ///
    /// Returns [`RuleMatch`] rather than a `bool`: a rule whose conditions were
    /// unevaluable is neither matched nor unmatched until its `effect` is
    /// consulted (PROTOCOL_SPEC §6.1.1).
    fn matches_rule(
        &self,
        rule: &ACLRule,
        caller: &str,
        target: &str,
        ctx: Option<&Context<serde_json::Value>>,
    ) -> RuleMatch {
        if !Self::match_patterns(&rule.callers, caller, ctx) {
            return RuleMatch::NoMatch;
        }

        if !Self::match_patterns(&rule.targets, target, ctx) {
            return RuleMatch::NoMatch;
        }

        // Conditions check.
        if let Some(ref conditions) = rule.conditions {
            return match self.check_conditions(conditions, ctx) {
                ConditionOutcome::Satisfied => RuleMatch::Match,
                ConditionOutcome::Unsatisfied => RuleMatch::NoMatch,
                ConditionOutcome::Unevaluable => RuleMatch::Unevaluable,
            };
        }

        RuleMatch::Match
    }

    /// Run PROTOCOL_SPEC §6.1.4's precheck and report any faults into the
    /// per-call handler-error scope. Returns `true` when the rule is
    /// unevaluable on structural or registry grounds alone.
    ///
    /// This runs **before** §6.5's no-context check (§6.1.4 rule 1), which is
    /// what closes the bypass where `conditions: {mispelled: true}` on a `deny`
    /// rule passed traffic simply because the caller carried no identity. A
    /// rule that *passes* here and then finds no context still takes §6.5's
    /// path and does not match: `roles` is answerable in principle, and this
    /// caller merely supplied no input for it (§6.1.4 rule 2).
    fn precheck_failed(conditions: &serde_json::Value, path: PrecheckPath) -> bool {
        let faults: Vec<RuleFault> = precheck_conditions(conditions, path);
        if faults.is_empty() {
            return false;
        }
        for fault in &faults {
            crate::acl_handlers::report_condition_unevaluable_at(&fault.path, &fault.reason);
        }
        true
    }

    /// Match a list of patterns against a value.
    /// Supports compound operators: `$or` (any match) and `$not` (negate).
    fn match_patterns(
        patterns: &[String],
        value: &str,
        ctx: Option<&Context<serde_json::Value>>,
    ) -> bool {
        if patterns.is_empty() {
            return false;
        }

        let first = patterns[0].as_str();
        if first == "$or" {
            return patterns[1..]
                .iter()
                .any(|p| Self::match_acl_pattern_with_ctx(p, value, ctx));
        }
        if first == "$not" {
            if patterns.len() < 2 {
                return false;
            }
            return !Self::match_acl_pattern_with_ctx(&patterns[1], value, ctx);
        }

        // Standard OR: any pattern matches
        patterns
            .iter()
            .any(|p| Self::match_acl_pattern_with_ctx(p, value, ctx))
    }

    /// Pattern matching for ACL patterns. Handles `@external`, `@system`, and
    /// delegates to `match_pattern()` for wildcard/glob matching.
    fn match_acl_pattern(pattern: &str, value: &str) -> bool {
        if pattern == "@external" {
            return value == "@external";
        }
        // @system is handled in match_acl_pattern_with_ctx (needs identity check)
        if pattern == "@system" {
            return false; // caller string is never literally "@system"
        }
        match_pattern(pattern, value)
    }

    fn match_acl_pattern_with_ctx(
        pattern: &str,
        value: &str,
        ctx: Option<&Context<serde_json::Value>>,
    ) -> bool {
        if pattern == "@system" {
            return ctx
                .and_then(|c| c.identity.as_ref())
                .is_some_and(|id| id.identity_type() == "system");
        }
        Self::match_acl_pattern(pattern, value)
    }

    /// Evaluate conditions block against the context using registered handlers.
    ///
    /// A missing context is `Unsatisfied`, NOT `Unevaluable`: PROTOCOL_SPEC
    /// §6.5 keeps "conditions present but no context provided" an ordinary
    /// non-match on purpose, because calling with no context is a legitimate
    /// shape for external entry points rather than a misconfiguration. Treating
    /// it as an evaluation failure would flip the decision for every
    /// `@external` call that meets a conditional `deny` rule.
    #[allow(clippy::unused_self)] // method must be on `&self` for trait-object dispatch consistency
    fn check_conditions(
        &self,
        conditions: &serde_json::Value,
        ctx: Option<&Context<serde_json::Value>>,
    ) -> ConditionOutcome {
        // §6.1.4 rule 1: the precheck is context-independent and runs FIRST.
        if Self::precheck_failed(conditions, PrecheckPath::Sync) {
            return ConditionOutcome::Unevaluable;
        }

        let Some(ctx) = ctx else {
            return ConditionOutcome::Unsatisfied; // §6.5: conditions require context
        };

        // The precheck already established that `conditions` is a mapping.
        let Some(obj) = conditions.as_object() else {
            return ConditionOutcome::Unevaluable;
        };

        let map: HashMap<String, serde_json::Value> =
            obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect();

        Self::evaluate_conditions(&map, ctx)
    }

    /// Async counterpart to `check_conditions`. Drives async condition handlers
    /// via `evaluate_conditions_async_outcome` so handlers that genuinely
    /// suspend are awaited rather than reported unevaluable.
    #[allow(clippy::unused_self)] // method must be on `&self` for trait-object dispatch consistency
    async fn check_conditions_async(
        &self,
        conditions: &serde_json::Value,
        ctx: Option<&Context<serde_json::Value>>,
    ) -> ConditionOutcome {
        // §6.1.4 rule 1, on the async path's registries (§6.1.3).
        if Self::precheck_failed(conditions, PrecheckPath::Async) {
            return ConditionOutcome::Unevaluable;
        }

        let Some(ctx) = ctx else {
            return ConditionOutcome::Unsatisfied; // §6.5
        };

        // The precheck already established that `conditions` is a mapping.
        let Some(obj) = conditions.as_object() else {
            return ConditionOutcome::Unevaluable;
        };

        let map: HashMap<String, serde_json::Value> =
            obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect();

        Self::evaluate_conditions_async_outcome(&map, ctx).await
    }

    /// Audit + return for the empty-rules path. Shared by `check` and `async_check`.
    ///
    /// `default_effect` is passed in as a parameter (rather than re-read from
    /// `self`) so that callers can supply a consistent snapshot taken at the
    /// entry of the check, eliminating TOCTOU drift if a concurrent
    /// add_rule/reload mutates the ACL during evaluation (sync finding A-D-301).
    fn finalize_no_rules(
        &self,
        default_effect: &str,
        caller: &str,
        target_id: &str,
        ctx: Option<&Context<serde_json::Value>>,
    ) -> bool {
        let entry = self.build_audit_entry(
            caller,
            target_id,
            default_effect,
            "no_rules",
            None,
            None,
            ctx,
        );
        self.emit_audit(&entry);
        default_effect == "allow"
    }

    /// Audit + return for a matched rule. Shared by `check` and `async_check`.
    fn finalize_rule_match(
        &self,
        idx: usize,
        rule: &ACLRule,
        caller: &str,
        target_id: &str,
        ctx: Option<&Context<serde_json::Value>>,
    ) -> bool {
        let entry = self.build_audit_entry(
            caller,
            target_id,
            &rule.effect,
            "rule_match",
            rule.description.as_deref(),
            Some(idx),
            ctx,
        );
        self.emit_audit(&entry);
        rule.effect == "allow"
    }

    /// Audit + return for the no-rule-matched path. Shared by `check` and
    /// `async_check`.
    ///
    /// `default_effect` is passed in as a parameter — see
    /// [`Self::finalize_no_rules`] for rationale (sync finding A-D-301).
    fn finalize_default_effect(
        &self,
        default_effect: &str,
        caller: &str,
        target_id: &str,
        ctx: Option<&Context<serde_json::Value>>,
    ) -> bool {
        let entry = self.build_audit_entry(
            caller,
            target_id,
            default_effect,
            "default_effect",
            None,
            None,
            ctx,
        );
        self.emit_audit(&entry);
        default_effect == "allow"
    }

    /// Build an audit entry from the check parameters and context.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::unused_self)] // method must be on `&self` for trait-object dispatch consistency
    fn build_audit_entry(
        &self,
        caller_id: &str,
        target_id: &str,
        decision: &str,
        reason: &str,
        matched_rule_desc: Option<&str>,
        matched_rule_index: Option<usize>,
        ctx: Option<&Context<serde_json::Value>>,
    ) -> AuditEntry {
        AuditEntry {
            timestamp: Utc::now().to_rfc3339(),
            caller_id: caller_id.to_string(),
            target_id: target_id.to_string(),
            decision: decision.to_string(),
            reason: reason.to_string(),
            matched_rule: matched_rule_desc.map(std::string::ToString::to_string),
            matched_rule_index,
            identity_type: ctx
                .and_then(|c| c.identity.as_ref().map(|id| id.identity_type().to_string())),
            roles: ctx
                .and_then(|c| c.identity.as_ref().map(|id| id.roles().to_vec()))
                .unwrap_or_default(),
            call_depth: ctx.map(|c| c.call_chain.len()),
            trace_id: ctx.map(|c| c.trace_id.clone()),
            // Read the per-call handler-error slot populated by
            // `acl_handlers::report_handler_error`. Consults the async
            // task-local scope (from `async_check`) and the synchronous
            // thread-local scope (from `check`), so a handler error populates
            // the audit entry on BOTH paths. Returns `None` when no handler
            // reported an error or when built outside a capture scope. Mirrors
            // Python `_handler_error_var.get()` and TypeScript
            // `_lastHandlerError` (closes A-D-ACL-001 / A-D-002).
            handler_error: crate::acl_handlers::current_handler_error(),
        }
    }

    /// Async check whether the given caller is allowed to access the target.
    /// Uses first-match-wins evaluation with async condition handler support.
    ///
    /// Async entry point. Shares all audit construction with `check` via the
    /// `finalize_*` helpers so the two methods cannot drift on logging fields,
    /// reason strings, or default-effect mapping.
    pub async fn async_check(
        &self,
        caller_id: Option<&str>,
        target_id: &str,
        ctx: Option<&Context<serde_json::Value>>,
    ) -> bool {
        // Wrap the entire evaluation in a per-call handler-error capture
        // scope so any handler that calls `report_handler_error(...)` lands
        // its message in this call's audit entry. The captured value is
        // read by `build_audit_entry` via the `HANDLER_ERROR` task-local
        // (closes A-D-ACL-001).
        let (decision, _captured) = crate::acl_handlers::with_handler_error_capture(
            self.async_check_inner(caller_id, target_id, ctx),
        )
        .await;
        decision
    }

    async fn async_check_inner(
        &self,
        caller_id: Option<&str>,
        target_id: &str,
        ctx: Option<&Context<serde_json::Value>>,
    ) -> bool {
        let caller = caller_id.unwrap_or("@external");

        // Snapshot rules + default_effect at entry so any concurrent mutator
        // (e.g., another task calling add_rule/reload through an
        // Arc<RwLock<ACL>> wrapper) cannot cause TOCTOU drift mid-evaluation.
        // Mirrors the sync `check()` snapshot and apcore-python /
        // apcore-typescript async paths (sync finding A-D-301).
        let rules: Vec<ACLRule> = self.rules.clone();
        let default_effect: String = self.default_effect.clone();

        if rules.is_empty() {
            return self.finalize_no_rules(&default_effect, caller, target_id, ctx);
        }

        for (idx, rule) in rules.iter().enumerate() {
            let paths_before = crate::acl_handlers::reported_condition_paths();
            match self.matches_rule_async(rule, caller, target_id, ctx).await {
                RuleMatch::Match => {
                    return self.finalize_rule_match(idx, rule, caller, target_id, ctx)
                }
                RuleMatch::Unevaluable => {
                    if Self::resolve_unevaluable_rule(idx, rule, &paths_before) {
                        return self.finalize_rule_match(idx, rule, caller, target_id, ctx);
                    }
                }
                RuleMatch::NoMatch => {}
            }
        }

        self.finalize_default_effect(&default_effect, caller, target_id, ctx)
    }

    /// Async version of `matches_rule` that awaits async condition handlers.
    /// Mirrors the sync `matches_rule` exactly except it routes condition
    /// evaluation through `check_conditions_async` so async handlers are awaited
    /// rather than polled-once.
    async fn matches_rule_async(
        &self,
        rule: &ACLRule,
        caller: &str,
        target: &str,
        ctx: Option<&Context<serde_json::Value>>,
    ) -> RuleMatch {
        if !Self::match_patterns(&rule.callers, caller, ctx) {
            return RuleMatch::NoMatch;
        }

        if !Self::match_patterns(&rule.targets, target, ctx) {
            return RuleMatch::NoMatch;
        }

        if let Some(ref conditions) = rule.conditions {
            return match self.check_conditions_async(conditions, ctx).await {
                ConditionOutcome::Satisfied => RuleMatch::Match,
                ConditionOutcome::Unsatisfied => RuleMatch::NoMatch,
                ConditionOutcome::Unevaluable => RuleMatch::Unevaluable,
            };
        }

        RuleMatch::Match
    }

    /// Emit an audit entry to the registered audit logger, if any.
    fn emit_audit(&self, entry: &AuditEntry) {
        if let Some(ref logger) = self.audit_logger {
            logger(entry);
        }
    }

    /// Initialize built-in handlers. Call once during application startup.
    pub fn init_builtin_handlers() {
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            register_builtin_handlers();
        });
    }
}

impl Default for ACL {
    fn default() -> Self {
        Self::new(vec![], "deny", None)
    }
}

#[cfg(test)]
mod reload_tests {
    use super::*;
    use std::io::Write;

    // [acl-reload-yamlpath] reload() must leave `yaml_path` unchanged (it
    // re-reads the stored path). Matches apcore-python / apcore-typescript,
    // which do not re-set the path on reload.
    #[test]
    fn reload_leaves_yaml_path_unchanged() {
        let mut tmp = tempfile::NamedTempFile::new().expect("create tempfile");
        writeln!(tmp, "default_effect: deny\nrules: []\n").expect("write tempfile");
        let path = tmp.path().to_str().expect("utf8").to_string();

        let mut acl = ACL::load(&path).expect("initial load");
        let before = acl.yaml_path.clone();
        assert_eq!(before, Some(path.clone()));

        acl.reload().expect("reload");
        assert_eq!(
            acl.yaml_path, before,
            "reload must not change the stored yaml_path"
        );
    }
}

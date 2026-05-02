// APCore Protocol — Access Control Lists
// Spec reference: ACL rules, audit entries

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_yaml_ng as serde_yaml;
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Once};

use crate::acl_handlers::{
    evaluate_conditions_async as handlers_evaluate_conditions_async, register_builtin_handlers,
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
        // (sync finding A-D-027).
        Self::init_builtin_handlers();
        Self {
            rules,
            default_effect: default_effect.into(),
            yaml_path: None,
            audit_logger,
        }
    }

    /// Set the audit logger callback.
    pub fn set_audit_logger(&mut self, logger: impl Fn(&AuditEntry) + Send + Sync + 'static) {
        self.audit_logger = Some(Arc::new(logger));
    }

    /// Evaluate all conditions with AND logic using the handler registry. Fail-closed on unknown.
    ///
    /// This is a **sync** function. It drives each handler's future by polling it once with a
    /// noop waker. If the handler future is `Pending` on the first poll (i.e., the handler is
    /// genuinely async and needs to yield), the condition is treated as **unsatisfied** and a
    /// `tracing::warn!` is emitted. This is the correct fail-closed behaviour for a synchronous
    /// ACL gate, but callers should prefer [`ACL::async_check`] / [`Self::evaluate_conditions_async`]
    /// for any handler that may perform I/O. Registering an async handler that returns `Pending`
    /// and using `check()` will silently deny the call.
    ///
    /// **Architecture note:** two parallel paths exist — this sync path and the async
    /// [`Self::evaluate_conditions_async`]. Keep both in sync when adding new condition logic to avoid
    /// drift. New conditions should be tested against both paths.
    pub fn evaluate_conditions(
        conditions: &HashMap<String, serde_json::Value>,
        ctx: &Context<serde_json::Value>,
    ) -> bool {
        let mut to_evaluate = Vec::with_capacity(conditions.len());
        {
            let handlers = CONDITION_HANDLERS.read();
            for (key, value) in conditions {
                let handler = if let Some(h) = handlers.get(key.as_str()) {
                    h.clone()
                } else {
                    tracing::warn!("Unknown ACL condition '{}' — treated as unsatisfied", key);
                    return false;
                };
                to_evaluate.push((key, handler, value));
            }
        }

        for (key, handler, value) in to_evaluate {
            // Built-in handlers are trivially async (return immediately).
            // We poll the future once — if it's not ready, treat as unsatisfied.
            let fut = handler.evaluate(value, ctx);
            let fut = std::pin::pin!(fut);
            let waker = std::task::Waker::noop();
            let mut cx = std::task::Context::from_waker(waker);
            let result = match fut.poll(&mut cx) {
                std::task::Poll::Ready(val) => val,
                std::task::Poll::Pending => {
                    tracing::warn!(
                        "Async condition '{}' not immediately ready in sync context — treated as unsatisfied",
                        key,
                    );
                    return false;
                }
            };
            if !result {
                return false;
            }
        }
        true
    }

    /// Async evaluate all conditions with AND logic using the handler registry.
    ///
    /// Delegates to `acl_handlers::evaluate_conditions_async` so compound
    /// operators (`$or`, `$not`) share the same async evaluation path.
    pub async fn evaluate_conditions_async(
        conditions: &HashMap<String, serde_json::Value>,
        ctx: &Context<serde_json::Value>,
    ) -> bool {
        handlers_evaluate_conditions_async(conditions, ctx).await
    }

    /// Add a rule to the ACL (inserted at position 0, highest priority).
    pub fn add_rule(&mut self, rule: ACLRule) -> Result<(), ModuleError> {
        self.rules.insert(0, rule);
        Ok(())
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
            if self.matches_rule(rule, caller, target_id, ctx) {
                return self.finalize_rule_match(idx, rule, caller, target_id, ctx);
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

        let rules: Vec<ACLRule> = serde_json::from_value(rules_val.clone()).map_err(|e| {
            ModuleError::new(
                ErrorCode::ACLRuleError,
                format!("Invalid ACL rules in '{path}': {e}"),
            )
        })?;

        let default_effect = raw
            .get("default_effect")
            .and_then(|v| v.as_str())
            .unwrap_or("deny")
            .to_string();

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

    /// Register a custom condition handler. Delegates to `acl_handlers::register_condition`.
    pub fn register_condition(
        key: impl Into<String>,
        handler: std::sync::Arc<dyn crate::acl_handlers::ACLConditionHandler>,
    ) {
        crate::acl_handlers::register_condition(key, handler);
    }

    /// Register a custom async-capable condition handler.
    ///
    /// In Rust all handlers are structurally async (`ACLConditionHandler::evaluate`
    /// is `async fn`), so this is an alias for [`ACL::register_condition`].
    /// The alias exists for cross-language API parity with apcore-python
    /// `register_async_condition` and apcore-typescript `registerAsyncCondition`
    /// (sync finding A-D-022).
    pub fn register_async_condition(
        key: impl Into<String>,
        handler: std::sync::Arc<dyn crate::acl_handlers::ACLConditionHandler>,
    ) {
        Self::register_condition(key, handler);
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
                    ErrorCode::ReloadFailed,
                    "Cannot reload: no yaml_path stored".to_string(),
                )
            })?
        };

        // File I/O happens here with no outstanding borrow on `self`.
        let reloaded = Self::load(&path)?;

        self.rules = reloaded.rules;
        self.default_effect = reloaded.default_effect;
        self.yaml_path = reloaded.yaml_path;
        Ok(())
    }

    /// Return a reference to the current rules.
    ///
    /// Returns a snapshot of the current rules.
    #[must_use]
    pub fn rules(&self) -> &[ACLRule] {
        &self.rules
    }

    // --- Private helpers ---

    /// Check if a rule matches the caller, target, and context.
    fn matches_rule(
        &self,
        rule: &ACLRule,
        caller: &str,
        target: &str,
        ctx: Option<&Context<serde_json::Value>>,
    ) -> bool {
        if !Self::match_patterns(&rule.callers, caller, ctx) {
            return false;
        }

        if !Self::match_patterns(&rule.targets, target, ctx) {
            return false;
        }

        // Conditions check.
        if let Some(ref conditions) = rule.conditions {
            if !self.check_conditions(conditions, ctx) {
                return false;
            }
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
    #[allow(clippy::unused_self)] // method must be on `&self` for trait-object dispatch consistency
    fn check_conditions(
        &self,
        conditions: &serde_json::Value,
        ctx: Option<&Context<serde_json::Value>>,
    ) -> bool {
        let Some(ctx) = ctx else {
            return false; // conditions require context
        };

        let Some(obj) = conditions.as_object() else {
            return false;
        };

        let map: HashMap<String, serde_json::Value> =
            obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect();

        Self::evaluate_conditions(&map, ctx)
    }

    /// Async counterpart to `check_conditions`. Drives async condition handlers
    /// via `evaluate_conditions_async` so handlers that genuinely suspend are
    /// awaited rather than treated as unsatisfied.
    #[allow(clippy::unused_self)] // method must be on `&self` for trait-object dispatch consistency
    async fn check_conditions_async(
        &self,
        conditions: &serde_json::Value,
        ctx: Option<&Context<serde_json::Value>>,
    ) -> bool {
        let Some(ctx) = ctx else {
            return false;
        };

        let Some(obj) = conditions.as_object() else {
            return false;
        };

        let map: HashMap<String, serde_json::Value> =
            obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect();

        Self::evaluate_conditions_async(&map, ctx).await
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
            handler_error: None,
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
            if self.matches_rule_async(rule, caller, target_id, ctx).await {
                return self.finalize_rule_match(idx, rule, caller, target_id, ctx);
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
    ) -> bool {
        if !Self::match_patterns(&rule.callers, caller, ctx) {
            return false;
        }

        if !Self::match_patterns(&rule.targets, target, ctx) {
            return false;
        }

        if let Some(ref conditions) = rule.conditions {
            if !self.check_conditions_async(conditions, ctx).await {
                return false;
            }
        }

        true
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

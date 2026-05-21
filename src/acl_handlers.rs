// Built-in ACL condition handlers and handler trait.
//
// Defines the ACLConditionHandler trait, three basic handlers
// (identity_types, roles, max_call_depth), and two compound operators ($or, $not).

use async_trait::async_trait;
use parking_lot::RwLock;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use crate::context::Context;

// Per-call slot for the latest handler error message. Mirrors Python's
// `_handler_error_var` ContextVar and TypeScript's `_lastHandlerError`
// — a handler that detects an internal failure can record it here, and
// `ACL::build_audit_entry` reads it back when emitting the audit record.
//
// Stored as a task-local so concurrent ACL evaluations on different tokio
// tasks do not see each other's errors. The cell defaults to `None`; the
// `with_handler_error_capture` helper wraps an async evaluation in a fresh
// slot so the read at the end of the call sees only that call's error.
tokio::task_local! {
    pub(crate) static HANDLER_ERROR: RefCell<Option<String>>;
}

/// Record a handler-evaluation error for the current ACL check.
///
/// Cross-language parity with apcore-python `_handler_error_var.set(...)` and
/// apcore-typescript `_lastHandlerError = ...`. If called outside an active
/// `with_handler_error_capture` scope (i.e. from synchronous evaluation paths
/// or outside an ACL check entirely), the call is a no-op so handlers never
/// panic on a missing task-local.
pub fn report_handler_error(message: impl Into<String>) {
    let msg = message.into();
    let _ = HANDLER_ERROR.try_with(|cell| {
        *cell.borrow_mut() = Some(msg);
    });
}

/// Run an async evaluation under a fresh handler-error capture scope.
///
/// The scope's final `Option<String>` is returned alongside the evaluation
/// result so callers can attach it to an `AuditEntry`. Mirrors the Python
/// `with _handler_error_var.set(None)` context-manager pattern.
pub async fn with_handler_error_capture<F, T>(fut: F) -> (T, Option<String>)
where
    F: std::future::Future<Output = T>,
{
    let cell = RefCell::new(None);
    let result = HANDLER_ERROR.scope(cell, async move {
        let value = fut.await;
        let captured = HANDLER_ERROR.with(|c| c.borrow().clone());
        (value, captured)
    });
    result.await
}

/// Trait for evaluating a single ACL condition.
#[async_trait]
pub trait ACLConditionHandler: Send + Sync {
    async fn evaluate(&self, value: &Value, ctx: &Context<Value>) -> bool;
}

/// Global registry of condition handlers (sync entry point — consulted by
/// `ACL::check` and as the fallback for `async_check`).
pub static CONDITION_HANDLERS: LazyLock<RwLock<HashMap<String, Arc<dyn ACLConditionHandler>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Separate registry for handlers explicitly registered as async-only via
/// `ACL::register_async_condition`. `async_check` consults this map first
/// and falls back to [`CONDITION_HANDLERS`] when no async-specific handler
/// is registered for a key. Mirrors apcore-python `_async_condition_handlers`
/// and apcore-typescript `_asyncConditionHandlers` (closes A-D-ACL-002).
pub static ASYNC_CONDITION_HANDLERS: LazyLock<
    RwLock<HashMap<String, Arc<dyn ACLConditionHandler>>>,
> = LazyLock::new(|| RwLock::new(HashMap::new()));

/// Register a condition handler globally. Replaces any existing handler for the same key.
///
/// See also [`ACL::register_condition`](crate::ACL::register_condition) for the convenience
/// static method on the ACL type, which delegates here.
pub fn register_condition(key: impl Into<String>, handler: Arc<dyn ACLConditionHandler>) {
    let mut map = CONDITION_HANDLERS.write();
    map.insert(key.into(), handler);
}

/// Register an async-only condition handler.
///
/// Handlers registered here are consulted by [`evaluate_conditions_async`]
/// *before* the sync registry, allowing async-only logic to override a sync
/// handler with the same key without affecting the sync `ACL::check` path.
/// Cross-language parity with apcore-python `register_async_condition` and
/// apcore-typescript `registerAsyncCondition` (closes A-D-ACL-002).
pub fn register_async_condition(key: impl Into<String>, handler: Arc<dyn ACLConditionHandler>) {
    let mut map = ASYNC_CONDITION_HANDLERS.write();
    map.insert(key.into(), handler);
}

// ---------------------------------------------------------------------------
// Free async evaluator (used by compound operators and re-exported to ACL)
// ---------------------------------------------------------------------------

/// Evaluate all conditions with AND logic using the handler registry.
///
/// Resolves each condition key by consulting [`ASYNC_CONDITION_HANDLERS`]
/// first (async-only overrides), then falling back to [`CONDITION_HANDLERS`]
/// (the sync registry). Unknown keys are treated as unsatisfied
/// (fail-closed). Handlers are cloned out of the registries before any
/// `.await` so no `parking_lot` read guard is held across await points.
pub async fn evaluate_conditions_async<S: ::std::hash::BuildHasher>(
    conditions: &HashMap<String, Value, S>,
    ctx: &Context<Value>,
) -> bool {
    let mut to_evaluate: Vec<(Arc<dyn ACLConditionHandler>, Value)> =
        Vec::with_capacity(conditions.len());
    {
        let async_handlers = ASYNC_CONDITION_HANDLERS.read();
        let sync_handlers = CONDITION_HANDLERS.read();
        for (key, value) in conditions {
            let handler = if let Some(h) = async_handlers.get(key.as_str()) {
                h.clone()
            } else if let Some(h) = sync_handlers.get(key.as_str()) {
                h.clone()
            } else {
                tracing::warn!("Unknown ACL condition '{}' — treated as unsatisfied", key);
                return false;
            };
            to_evaluate.push((handler, value.clone()));
        }
    }
    for (handler, value) in &to_evaluate {
        if !handler.evaluate(value, ctx).await {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Basic handlers
// ---------------------------------------------------------------------------

/// Check context.identity.type is in the allowed list.
pub struct IdentityTypesHandler;

#[async_trait]
impl ACLConditionHandler for IdentityTypesHandler {
    async fn evaluate(&self, value: &Value, ctx: &Context<Value>) -> bool {
        let Some(arr) = value.as_array() else {
            return false;
        };
        let Some(identity) = &ctx.identity else {
            return false;
        };
        arr.iter()
            .any(|v| v.as_str().is_some_and(|s| s == identity.identity_type()))
    }
}

/// Check at least one role overlaps between identity and required roles.
pub struct RolesHandler;

#[async_trait]
impl ACLConditionHandler for RolesHandler {
    async fn evaluate(&self, value: &Value, ctx: &Context<Value>) -> bool {
        let Some(arr) = value.as_array() else {
            return false;
        };
        let Some(identity) = &ctx.identity else {
            return false;
        };
        arr.iter().any(|v| {
            v.as_str()
                .is_some_and(|s| identity.roles().contains(&s.to_string()))
        })
    }
}

/// Check call chain length does not exceed threshold.
///
/// Accepts both the bare-integer form `max_call_depth: 5` and the dict form
/// `max_call_depth: { lte: 5 }`, mirroring apcore-python and apcore-typescript
/// (sync finding A-D-024). Other forms are rejected (fail-closed) per spec.
pub struct MaxCallDepthHandler;

#[async_trait]
impl ACLConditionHandler for MaxCallDepthHandler {
    async fn evaluate(&self, value: &Value, ctx: &Context<Value>) -> bool {
        let threshold = match value {
            Value::Number(n) => n.as_u64(),
            Value::Object(map) => map.get("lte").and_then(serde_json::Value::as_u64),
            _ => None,
        };
        match threshold {
            Some(max) => (ctx.call_chain.len() as u64) <= max,
            None => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Compound handlers
// ---------------------------------------------------------------------------

/// $or: list of condition dicts. Returns true if ANY sub-set passes.
/// Delegates to `evaluate_conditions_async` so async handlers in sub-conditions
/// are fully awaited (fixes the prior sync-path footgun).
pub(crate) struct OrHandler;

impl OrHandler {
    pub(crate) fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ACLConditionHandler for OrHandler {
    async fn evaluate(&self, value: &Value, ctx: &Context<Value>) -> bool {
        let Some(arr) = value.as_array() else {
            return false;
        };
        for sub in arr {
            if let Some(obj) = sub.as_object() {
                let map: HashMap<String, Value> =
                    obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                if evaluate_conditions_async(&map, ctx).await {
                    return true;
                }
            }
        }
        false
    }
}

/// $not: single condition dict. Returns true if the sub-set FAILS.
/// Delegates to `evaluate_conditions_async` for the same reason as `OrHandler`.
pub(crate) struct NotHandler;

impl NotHandler {
    pub(crate) fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ACLConditionHandler for NotHandler {
    async fn evaluate(&self, value: &Value, ctx: &Context<Value>) -> bool {
        match value.as_object() {
            Some(obj) => {
                let map: HashMap<String, Value> =
                    obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                !evaluate_conditions_async(&map, ctx).await
            }
            None => false,
        }
    }
}

/// Register all built-in handlers. Called once during initialization.
pub fn register_builtin_handlers() {
    register_condition("identity_types", Arc::new(IdentityTypesHandler));
    register_condition("roles", Arc::new(RolesHandler));
    register_condition("max_call_depth", Arc::new(MaxCallDepthHandler));
    register_condition("$or", Arc::new(OrHandler::new()));
    register_condition("$not", Arc::new(NotHandler::new()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{Context, Identity};

    fn make_ctx(identity_type: &str, roles: Vec<&str>, call_depth: usize) -> Context<Value> {
        let identity = Identity::new(
            "test-id".to_string(),
            identity_type.to_string(),
            roles.into_iter().map(String::from).collect(),
            HashMap::new(),
        );
        let mut ctx = Context::new(identity);
        for i in 0..call_depth {
            ctx.call_chain.push(format!("module.{i}"));
        }
        ctx
    }

    fn anon_ctx() -> Context<Value> {
        Context::<Value>::anonymous()
    }

    // -------------------------------------------------------------------------
    // IdentityTypesHandler
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn identity_types_matches_correct_type() {
        let handler = IdentityTypesHandler;
        let ctx = make_ctx("user", vec![], 0);
        let value = serde_json::json!(["user", "service"]);
        assert!(handler.evaluate(&value, &ctx).await);
    }

    #[tokio::test]
    async fn identity_types_rejects_wrong_type() {
        let handler = IdentityTypesHandler;
        let ctx = make_ctx("agent", vec![], 0);
        let value = serde_json::json!(["user", "service"]);
        assert!(!handler.evaluate(&value, &ctx).await);
    }

    #[tokio::test]
    async fn identity_types_rejects_non_array_value() {
        let handler = IdentityTypesHandler;
        let ctx = make_ctx("user", vec![], 0);
        let value = serde_json::json!("user"); // not an array
        assert!(!handler.evaluate(&value, &ctx).await);
    }

    #[tokio::test]
    async fn identity_types_rejects_no_identity() {
        let handler = IdentityTypesHandler;
        let ctx = anon_ctx();
        let value = serde_json::json!(["user"]);
        assert!(!handler.evaluate(&value, &ctx).await);
    }

    // -------------------------------------------------------------------------
    // RolesHandler
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn roles_matches_overlapping_role() {
        let handler = RolesHandler;
        let ctx = make_ctx("user", vec!["admin", "viewer"], 0);
        let value = serde_json::json!(["admin"]);
        assert!(handler.evaluate(&value, &ctx).await);
    }

    #[tokio::test]
    async fn roles_rejects_no_overlap() {
        let handler = RolesHandler;
        let ctx = make_ctx("user", vec!["viewer"], 0);
        let value = serde_json::json!(["admin"]);
        assert!(!handler.evaluate(&value, &ctx).await);
    }

    #[tokio::test]
    async fn roles_rejects_no_identity() {
        let handler = RolesHandler;
        let ctx = anon_ctx();
        let value = serde_json::json!(["admin"]);
        assert!(!handler.evaluate(&value, &ctx).await);
    }

    // -------------------------------------------------------------------------
    // MaxCallDepthHandler
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn max_call_depth_allows_under_limit() {
        let handler = MaxCallDepthHandler;
        let ctx = make_ctx("user", vec![], 3);
        let value = serde_json::json!(5u64);
        assert!(handler.evaluate(&value, &ctx).await);
    }

    #[tokio::test]
    async fn max_call_depth_allows_at_limit() {
        let handler = MaxCallDepthHandler;
        let ctx = make_ctx("user", vec![], 5);
        let value = serde_json::json!(5u64);
        assert!(handler.evaluate(&value, &ctx).await);
    }

    #[tokio::test]
    async fn max_call_depth_rejects_over_limit() {
        let handler = MaxCallDepthHandler;
        let ctx = make_ctx("user", vec![], 6);
        let value = serde_json::json!(5u64);
        assert!(!handler.evaluate(&value, &ctx).await);
    }

    #[tokio::test]
    async fn max_call_depth_rejects_non_numeric_value() {
        let handler = MaxCallDepthHandler;
        let ctx = make_ctx("user", vec![], 0);
        let value = serde_json::json!("five"); // not a number
        assert!(!handler.evaluate(&value, &ctx).await);
    }

    // -------------------------------------------------------------------------
    // OrHandler
    // -------------------------------------------------------------------------

    /// Simple async handler for compound-operator tests: checks `{"pass": true}`.
    struct PassHandler;

    #[async_trait]
    impl ACLConditionHandler for PassHandler {
        async fn evaluate(&self, value: &Value, _ctx: &Context<Value>) -> bool {
            value.as_bool().unwrap_or(false)
        }
    }

    /// Register "pass" handler and ensure built-ins are present before compound tests.
    fn setup_compound_test_handlers() {
        register_condition("pass", Arc::new(PassHandler));
        // Ensure the OrHandler / NotHandler themselves are registered so nested
        // compound operators work in the integration tests below.
        register_builtin_handlers();
    }

    #[tokio::test]
    async fn or_handler_true_if_any_sub_passes() {
        setup_compound_test_handlers();
        let handler = OrHandler::new();
        let ctx = anon_ctx();
        let value = serde_json::json!([
            {"pass": false},
            {"pass": true},
        ]);
        assert!(handler.evaluate(&value, &ctx).await);
    }

    #[tokio::test]
    async fn or_handler_false_if_none_pass() {
        setup_compound_test_handlers();
        let handler = OrHandler::new();
        let ctx = anon_ctx();
        let value = serde_json::json!([
            {"pass": false},
            {"pass": false},
        ]);
        assert!(!handler.evaluate(&value, &ctx).await);
    }

    #[tokio::test]
    async fn or_handler_rejects_non_array_value() {
        let handler = OrHandler::new();
        let ctx = anon_ctx();
        let value = serde_json::json!({"pass": true}); // not an array
        assert!(!handler.evaluate(&value, &ctx).await);
    }

    // -------------------------------------------------------------------------
    // NotHandler
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn not_handler_inverts_passing_condition() {
        setup_compound_test_handlers();
        let handler = NotHandler::new();
        let ctx = anon_ctx();
        let value = serde_json::json!({"pass": true});
        assert!(!handler.evaluate(&value, &ctx).await);
    }

    #[tokio::test]
    async fn not_handler_inverts_failing_condition() {
        setup_compound_test_handlers();
        let handler = NotHandler::new();
        let ctx = anon_ctx();
        let value = serde_json::json!({"pass": false});
        assert!(handler.evaluate(&value, &ctx).await);
    }

    #[tokio::test]
    async fn not_handler_rejects_non_object_value() {
        let handler = NotHandler::new();
        let ctx = anon_ctx();
        let value = serde_json::json!([{"pass": true}]); // not an object
        assert!(!handler.evaluate(&value, &ctx).await);
    }

    // -------------------------------------------------------------------------
    // register_condition
    // -------------------------------------------------------------------------

    #[test]
    fn register_condition_stores_and_overwrites() {
        register_condition("_test_handler", Arc::new(MaxCallDepthHandler));
        // Overwrite — should not panic
        register_condition("_test_handler", Arc::new(MaxCallDepthHandler));
        let map = CONDITION_HANDLERS.read();
        assert!(map.contains_key("_test_handler"));
    }

    // -------------------------------------------------------------------------
    // register_async_condition — separate registry from sync (A-D-ACL-002)
    // -------------------------------------------------------------------------

    /// Async-only handler that always returns true.
    struct AsyncOnlyTrue;

    #[async_trait]
    impl ACLConditionHandler for AsyncOnlyTrue {
        async fn evaluate(&self, _value: &Value, _ctx: &Context<Value>) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn register_async_condition_uses_separate_registry() {
        // Register the same key in both registries with opposite outcomes —
        // the async path MUST consult the async registry first.
        struct SyncDeny;
        #[async_trait]
        impl ACLConditionHandler for SyncDeny {
            async fn evaluate(&self, _value: &Value, _ctx: &Context<Value>) -> bool {
                false
            }
        }

        let key = "_test_async_vs_sync";
        register_condition(key, Arc::new(SyncDeny));
        register_async_condition(key, Arc::new(AsyncOnlyTrue));

        // Async path resolves the async-only handler → true.
        let mut conditions: HashMap<String, Value> = HashMap::new();
        conditions.insert(key.to_string(), Value::Null);
        let ctx = anon_ctx();
        assert!(evaluate_conditions_async(&conditions, &ctx).await);

        // Sync registry still contains the deny handler.
        let sync_map = CONDITION_HANDLERS.read();
        assert!(sync_map.contains_key(key));
        let async_map = ASYNC_CONDITION_HANDLERS.read();
        assert!(async_map.contains_key(key));
    }

    #[tokio::test]
    async fn async_check_falls_back_to_sync_registry_when_no_async_handler() {
        // Only register on the sync side — async evaluation MUST still find it.
        let key = "_test_async_fallback";
        register_condition(key, Arc::new(AsyncOnlyTrue));

        let mut conditions: HashMap<String, Value> = HashMap::new();
        conditions.insert(key.to_string(), Value::Null);
        let ctx = anon_ctx();
        assert!(evaluate_conditions_async(&conditions, &ctx).await);
    }

    // -------------------------------------------------------------------------
    // handler_error capture (A-D-ACL-001)
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn handler_error_capture_returns_reported_message() {
        let (decision, captured) = with_handler_error_capture(async {
            report_handler_error("simulated handler failure");
            false
        })
        .await;
        assert!(!decision);
        assert_eq!(captured.as_deref(), Some("simulated handler failure"));
    }

    #[tokio::test]
    async fn handler_error_capture_isolates_per_scope() {
        // Two independent scopes must not see each other's errors.
        let ((), first) = with_handler_error_capture(async {
            report_handler_error("first call");
        })
        .await;
        let ((), second) = with_handler_error_capture(async {
            // No report inside this scope.
        })
        .await;
        assert_eq!(first.as_deref(), Some("first call"));
        assert!(second.is_none());
    }

    #[test]
    fn report_handler_error_outside_scope_is_noop() {
        // Calling outside an active capture scope must not panic — it falls
        // through silently because the task-local has no slot.
        report_handler_error("dropped on the floor");
    }
}

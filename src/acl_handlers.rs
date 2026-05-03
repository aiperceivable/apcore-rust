// Built-in ACL condition handlers and handler trait.
//
// Defines the ACLConditionHandler trait, three basic handlers
// (identity_types, roles, max_call_depth), and two compound operators ($or, $not).

use async_trait::async_trait;
use parking_lot::RwLock;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use crate::context::Context;

/// Trait for evaluating a single ACL condition.
#[async_trait]
pub trait ACLConditionHandler: Send + Sync {
    async fn evaluate(&self, value: &Value, ctx: &Context<Value>) -> bool;
}

/// Global registry of condition handlers.
pub static CONDITION_HANDLERS: LazyLock<RwLock<HashMap<String, Arc<dyn ACLConditionHandler>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Register a condition handler globally. Replaces any existing handler for the same key.
///
/// See also [`ACL::register_condition`](crate::ACL::register_condition) for the convenience
/// static method on the ACL type, which delegates here.
pub fn register_condition(key: impl Into<String>, handler: Arc<dyn ACLConditionHandler>) {
    let mut map = CONDITION_HANDLERS.write();
    map.insert(key.into(), handler);
}

// ---------------------------------------------------------------------------
// Free async evaluator (used by compound operators and re-exported to ACL)
// ---------------------------------------------------------------------------

/// Evaluate all conditions with AND logic using the handler registry.
/// Unknown condition keys are treated as unsatisfied (fail-closed).
/// Handlers are cloned out of the registry before any `.await` so no
/// `parking_lot` read guard is held across await points.
pub async fn evaluate_conditions_async<S: ::std::hash::BuildHasher>(
    conditions: &HashMap<String, Value, S>,
    ctx: &Context<Value>,
) -> bool {
    let mut to_evaluate: Vec<(Arc<dyn ACLConditionHandler>, Value)> =
        Vec::with_capacity(conditions.len());
    {
        let handlers = CONDITION_HANDLERS.read();
        for (key, value) in conditions {
            let handler = if let Some(h) = handlers.get(key.as_str()) {
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
}

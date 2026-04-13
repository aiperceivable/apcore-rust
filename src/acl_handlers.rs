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
pub fn register_condition(key: impl Into<String>, handler: Arc<dyn ACLConditionHandler>) {
    let mut map = CONDITION_HANDLERS.write();
    map.insert(key.into(), handler);
}

/// Type alias for the sync evaluation function used by compound handlers.
pub(crate) type EvalFn = fn(&HashMap<String, Value>, &Context<Value>) -> bool;

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
pub struct MaxCallDepthHandler;

#[async_trait]
impl ACLConditionHandler for MaxCallDepthHandler {
    async fn evaluate(&self, value: &Value, ctx: &Context<Value>) -> bool {
        match value.as_u64() {
            Some(max) => (ctx.call_chain.len() as u64) <= max,
            None => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Compound handlers
// ---------------------------------------------------------------------------

/// $or: list of condition dicts. Returns true if ANY sub-set passes.
pub(crate) struct OrHandler {
    evaluate_fn: EvalFn,
}

impl OrHandler {
    pub(crate) fn new(evaluate_fn: EvalFn) -> Self {
        Self { evaluate_fn }
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
                if (self.evaluate_fn)(&map, ctx) {
                    return true;
                }
            }
        }
        false
    }
}

/// $not: single condition dict. Returns true if the sub-set FAILS.
pub(crate) struct NotHandler {
    evaluate_fn: EvalFn,
}

impl NotHandler {
    pub(crate) fn new(evaluate_fn: EvalFn) -> Self {
        Self { evaluate_fn }
    }
}

#[async_trait]
impl ACLConditionHandler for NotHandler {
    async fn evaluate(&self, value: &Value, ctx: &Context<Value>) -> bool {
        match value.as_object() {
            Some(obj) => {
                let map: HashMap<String, Value> =
                    obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                !(self.evaluate_fn)(&map, ctx)
            }
            None => false,
        }
    }
}

/// Register all built-in handlers. Called once during initialization.
pub fn register_builtin_handlers(evaluate_fn: EvalFn) {
    register_condition("identity_types", Arc::new(IdentityTypesHandler));
    register_condition("roles", Arc::new(RolesHandler));
    register_condition("max_call_depth", Arc::new(MaxCallDepthHandler));
    register_condition("$or", Arc::new(OrHandler::new(evaluate_fn)));
    register_condition("$not", Arc::new(NotHandler::new(evaluate_fn)));
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

    fn simple_eval(conditions: &HashMap<String, Value>, _ctx: &Context<Value>) -> bool {
        // Evaluates "pass: true" condition for testing purposes
        conditions.get("pass").and_then(Value::as_bool).unwrap_or(false)
    }

    #[tokio::test]
    async fn or_handler_true_if_any_sub_passes() {
        let handler = OrHandler::new(simple_eval);
        let ctx = anon_ctx();
        let value = serde_json::json!([
            {"pass": false},
            {"pass": true},
        ]);
        assert!(handler.evaluate(&value, &ctx).await);
    }

    #[tokio::test]
    async fn or_handler_false_if_none_pass() {
        let handler = OrHandler::new(simple_eval);
        let ctx = anon_ctx();
        let value = serde_json::json!([
            {"pass": false},
            {"pass": false},
        ]);
        assert!(!handler.evaluate(&value, &ctx).await);
    }

    #[tokio::test]
    async fn or_handler_rejects_non_array_value() {
        let handler = OrHandler::new(simple_eval);
        let ctx = anon_ctx();
        let value = serde_json::json!({"pass": true}); // not an array
        assert!(!handler.evaluate(&value, &ctx).await);
    }

    // -------------------------------------------------------------------------
    // NotHandler
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn not_handler_inverts_passing_condition() {
        let handler = NotHandler::new(simple_eval);
        let ctx = anon_ctx();
        let value = serde_json::json!({"pass": true});
        assert!(!handler.evaluate(&value, &ctx).await);
    }

    #[tokio::test]
    async fn not_handler_inverts_failing_condition() {
        let handler = NotHandler::new(simple_eval);
        let ctx = anon_ctx();
        let value = serde_json::json!({"pass": false});
        assert!(handler.evaluate(&value, &ctx).await);
    }

    #[tokio::test]
    async fn not_handler_rejects_non_object_value() {
        let handler = NotHandler::new(simple_eval);
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

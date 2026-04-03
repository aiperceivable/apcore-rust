// Built-in ACL condition handlers and handler trait.
//
// Defines the ACLConditionHandler trait, three basic handlers
// (identity_types, roles, max_call_depth), and two compound operators ($or, $not).

use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

use crate::context::Context;

/// Trait for evaluating a single ACL condition.
#[async_trait]
pub trait ACLConditionHandler: Send + Sync {
    async fn evaluate(&self, value: &Value, ctx: &Context<Value>) -> bool;
}

/// Global registry of condition handlers.
pub static CONDITION_HANDLERS: LazyLock<RwLock<HashMap<String, Box<dyn ACLConditionHandler>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Register a condition handler globally. Replaces any existing handler for the same key.
pub fn register_condition(key: impl Into<String>, handler: Box<dyn ACLConditionHandler>) {
    if let Ok(mut map) = CONDITION_HANDLERS.write() {
        map.insert(key.into(), handler);
    }
}

/// Type alias for the sync evaluation function used by compound handlers.
pub type EvalFn = fn(&HashMap<String, Value>, &Context<Value>) -> bool;

// ---------------------------------------------------------------------------
// Basic handlers
// ---------------------------------------------------------------------------

/// Check context.identity.type is in the allowed list.
pub struct IdentityTypesHandler;

#[async_trait]
impl ACLConditionHandler for IdentityTypesHandler {
    async fn evaluate(&self, value: &Value, ctx: &Context<Value>) -> bool {
        let arr = match value.as_array() {
            Some(a) => a,
            None => return false,
        };
        let identity = match &ctx.identity {
            Some(id) => id,
            None => return false,
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
        let arr = match value.as_array() {
            Some(a) => a,
            None => return false,
        };
        let identity = match &ctx.identity {
            Some(id) => id,
            None => return false,
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
pub struct OrHandler {
    evaluate_fn: EvalFn,
}

impl OrHandler {
    pub fn new(evaluate_fn: EvalFn) -> Self {
        Self { evaluate_fn }
    }
}

#[async_trait]
impl ACLConditionHandler for OrHandler {
    async fn evaluate(&self, value: &Value, ctx: &Context<Value>) -> bool {
        let arr = match value.as_array() {
            Some(a) => a,
            None => return false,
        };
        for sub in arr {
            if let Some(obj) = sub.as_object() {
                let map: HashMap<String, Value> = obj
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                if (self.evaluate_fn)(&map, ctx) {
                    return true;
                }
            }
        }
        false
    }
}

/// $not: single condition dict. Returns true if the sub-set FAILS.
pub struct NotHandler {
    evaluate_fn: EvalFn,
}

impl NotHandler {
    pub fn new(evaluate_fn: EvalFn) -> Self {
        Self { evaluate_fn }
    }
}

#[async_trait]
impl ACLConditionHandler for NotHandler {
    async fn evaluate(&self, value: &Value, ctx: &Context<Value>) -> bool {
        match value.as_object() {
            Some(obj) => {
                let map: HashMap<String, Value> = obj
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                !(self.evaluate_fn)(&map, ctx)
            }
            None => false,
        }
    }
}

/// Register all built-in handlers. Called once during initialization.
pub fn register_builtin_handlers(evaluate_fn: EvalFn) {
    register_condition("identity_types", Box::new(IdentityTypesHandler));
    register_condition("roles", Box::new(RolesHandler));
    register_condition("max_call_depth", Box::new(MaxCallDepthHandler));
    register_condition("$or", Box::new(OrHandler::new(evaluate_fn)));
    register_condition("$not", Box::new(NotHandler::new(evaluate_fn)));
}

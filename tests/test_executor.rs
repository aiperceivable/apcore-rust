//! Tests for Executor, MiddlewareManager, and APCore client integration.

use apcore::context::Context;
use apcore::errors::{ErrorCode, ModuleError};
use apcore::middleware::base::Middleware;
use apcore::module::Module;
use apcore::APCore;
use async_trait::async_trait;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Test module
// ---------------------------------------------------------------------------

struct AddModule;

#[async_trait]
impl Module for AddModule {
    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {"a": {"type": "integer"}, "b": {"type": "integer"}}})
    }
    fn output_schema(&self) -> Value {
        json!({"type": "object", "properties": {"result": {"type": "integer"}}})
    }
    fn description(&self) -> &str {
        "Add two numbers"
    }
    async fn execute(&self, inputs: Value, _ctx: &Context<Value>) -> Result<Value, ModuleError> {
        let a = inputs["a"].as_i64().unwrap_or(0);
        let b = inputs["b"].as_i64().unwrap_or(0);
        Ok(json!({"result": a + b}))
    }
}

// ---------------------------------------------------------------------------
// Test middleware
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct PrefixMiddleware;

#[async_trait]
impl Middleware for PrefixMiddleware {
    fn name(&self) -> &str {
        "prefix"
    }
    async fn before(
        &self,
        _module_id: &str,
        mut inputs: Value,
        _ctx: &Context<Value>,
    ) -> Result<Option<Value>, ModuleError> {
        // Add a prefix field to prove before() ran
        inputs["_prefixed"] = json!(true);
        Ok(Some(inputs))
    }
    async fn after(
        &self,
        _module_id: &str,
        _inputs: Value,
        mut output: Value,
        _ctx: &Context<Value>,
    ) -> Result<Option<Value>, ModuleError> {
        // Add a suffix field to prove after() ran
        output["_suffixed"] = json!(true);
        Ok(Some(output))
    }
    async fn on_error(
        &self,
        _module_id: &str,
        _inputs: Value,
        _error: &ModuleError,
        _ctx: &Context<Value>,
    ) -> Result<Option<Value>, ModuleError> {
        Ok(None)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_apcore_register_and_call() {
    let mut client = APCore::new();
    client.register("math.add", Box::new(AddModule)).unwrap();

    let result = client
        .call("math.add", json!({"a": 10, "b": 5}), None, None)
        .await
        .unwrap();

    assert_eq!(result["result"], 15);
}

#[tokio::test]
async fn test_apcore_call_missing_module() {
    let client = APCore::new();
    let err = client
        .call("nonexistent", json!({}), None, None)
        .await
        .unwrap_err();

    assert_eq!(err.code, ErrorCode::ModuleNotFound);
}

#[tokio::test]
async fn test_apcore_middleware_before_and_after() {
    let mut client = APCore::new();
    client.register("math.add", Box::new(AddModule)).unwrap();
    client.use_middleware(Box::new(PrefixMiddleware)).unwrap();

    let result = client
        .call("math.add", json!({"a": 1, "b": 2}), None, None)
        .await
        .unwrap();

    // after() should have added _suffixed
    assert_eq!(result["_suffixed"], true);
    // result should still have the computation
    assert_eq!(result["result"], 3);
}

#[tokio::test]
async fn test_apcore_remove_middleware() {
    let mut client = APCore::new();
    client.register("math.add", Box::new(AddModule)).unwrap();
    client.use_middleware(Box::new(PrefixMiddleware)).unwrap();

    // Remove the middleware
    let removed = client.remove("prefix");
    assert!(removed);

    // Call without middleware — no _suffixed field
    let result = client
        .call("math.add", json!({"a": 1, "b": 2}), None, None)
        .await
        .unwrap();

    assert!(result.get("_suffixed").is_none());
    assert_eq!(result["result"], 3);
}

#[tokio::test]
async fn test_apcore_list_modules() {
    let mut client = APCore::new();
    client.register("math.add", Box::new(AddModule)).unwrap();

    let modules = client.list_modules(None, None);
    assert!(modules.contains(&"math.add".to_string()));
}

#[tokio::test]
async fn test_apcore_describe_module() {
    let mut client = APCore::new();
    client.register("math.add", Box::new(AddModule)).unwrap();

    let desc = client.describe("math.add");
    assert_eq!(desc, "Add two numbers");
}

#[tokio::test]
async fn test_apcore_registry_accessor() {
    let mut client = APCore::new();
    client.register("math.add", Box::new(AddModule)).unwrap();

    assert!(client.registry().has("math.add"));
    assert_eq!(client.registry().count(), 1);
}

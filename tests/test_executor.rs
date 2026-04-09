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

#[derive(Debug)]
struct TagMiddleware;

#[async_trait]
impl Middleware for TagMiddleware {
    fn name(&self) -> &str {
        "tag"
    }
    async fn before(
        &self,
        _module_id: &str,
        inputs: Value,
        _ctx: &Context<Value>,
    ) -> Result<Option<Value>, ModuleError> {
        Ok(Some(inputs))
    }
    async fn after(
        &self,
        _module_id: &str,
        _inputs: Value,
        mut output: Value,
        _ctx: &Context<Value>,
    ) -> Result<Option<Value>, ModuleError> {
        output["_tagged"] = json!(true);
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

#[tokio::test]
async fn test_apcore_with_components() {
    use apcore::config::Config;
    use apcore::registry::registry::Registry;

    let registry = Registry::new();
    // Verify with_components builds a working client from registry + config
    let config = Config::default();
    let mut client = APCore::with_components(registry, config);
    client.register("math.add", Box::new(AddModule)).unwrap();

    let result = client
        .call("math.add", json!({"a": 3, "b": 7}), None, None)
        .await
        .unwrap();
    assert_eq!(result["result"], 10);
}

#[tokio::test]
async fn test_apcore_disable_enable() {
    let mut client = APCore::new();
    client.register("math.add", Box::new(AddModule)).unwrap();

    // Disable the module
    client.disable("math.add", Some("testing")).unwrap();

    // Verify calling disabled module fails
    let err = client
        .call("math.add", json!({"a": 1, "b": 2}), None, None)
        .await;
    assert!(err.is_err(), "calling a disabled module should fail");

    // Re-enable the module
    client.enable("math.add", Some("test done")).unwrap();

    // Verify it works again
    let result = client
        .call("math.add", json!({"a": 1, "b": 2}), None, None)
        .await
        .unwrap();
    assert_eq!(result["result"], 3);
}

#[tokio::test]
async fn test_apcore_disable_nonexistent_module() {
    let mut client = APCore::new();
    let err = client.disable("nonexistent.module", None);
    assert!(err.is_err(), "disabling a nonexistent module should fail");
}

#[tokio::test]
async fn test_apcore_middleware_chaining() {
    let mut client = APCore::new();
    client.register("math.add", Box::new(AddModule)).unwrap();

    // Verify middleware methods are truly chainable via Result<&mut Self>
    client
        .use_middleware(Box::new(PrefixMiddleware))
        .unwrap()
        .use_middleware(Box::new(TagMiddleware))
        .unwrap();

    // Verify both middleware were applied
    let result = client
        .call("math.add", json!({"a": 1, "b": 2}), None, None)
        .await
        .unwrap();
    assert!(
        result.get("_suffixed").is_some(),
        "PrefixMiddleware after() should add _suffixed"
    );
    assert!(
        result.get("_tagged").is_some(),
        "TagMiddleware after() should add _tagged"
    );
}

#[tokio::test]
async fn test_apcore_list_modules_with_tags() {
    let client = APCore::new();

    // Verify list_modules accepts &[&str] for tags
    let modules = client.list_modules(Some(&["math"]), None);
    assert!(modules.is_empty());

    let modules = client.list_modules(None, Some("system."));
    assert!(modules.is_empty());
}

// Regression for sync finding A-002 — Executor.validate() must accept an
// optional context parameter per PROTOCOL_SPEC §12.2 line 6405. Aligns Rust
// signature with apcore-python and apcore-typescript.
#[tokio::test]
async fn test_validate_accepts_optional_context() {
    use apcore::context::{Context, Identity};

    let mut client = APCore::new();
    client.register("math.add", Box::new(AddModule)).unwrap();

    // Path 1: validate with no context — anonymous external context synthesized internally.
    let r1 = client
        .validate("math.add", &json!({"a": 1, "b": 2}), None)
        .await
        .unwrap();
    assert!(
        r1.valid,
        "validate(.., None) should pass for a valid module"
    );

    // Path 2: validate with an explicit context — call_chain checks see real caller state.
    let identity = Identity::new(
        "test_caller".to_string(),
        "user".to_string(),
        vec!["tester".to_string()],
        Default::default(),
    );
    let ctx: Context<serde_json::Value> = Context::new(identity);
    let r2 = client
        .validate("math.add", &json!({"a": 1, "b": 2}), Some(&ctx))
        .await
        .unwrap();
    assert!(
        r2.valid,
        "validate(.., Some(ctx)) should pass for a valid module"
    );
    assert_eq!(
        r1.checks.len(),
        r2.checks.len(),
        "context shape should not change the number of preflight checks executed"
    );
}

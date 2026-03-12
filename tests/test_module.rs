//! Tests for Module trait, ModuleAnnotations, and ModuleExample.

use apcore::context::{Context, Identity};
use apcore::errors::{ErrorCode, ModuleError};
use apcore::module::{Module, ModuleAnnotations, ModuleExample, PreflightResult};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Test modules
// ---------------------------------------------------------------------------

struct EchoModule;

#[async_trait]
impl Module for EchoModule {
    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": { "value": { "type": "string" } } })
    }
    fn output_schema(&self) -> Value {
        json!({ "type": "object", "properties": { "value": { "type": "string" } } })
    }
    fn description(&self) -> &str {
        "Echo the input value back"
    }
    async fn execute(
        &self,
        _ctx: &Context<Value>,
        input: Value,
    ) -> Result<Value, ModuleError> {
        Ok(input)
    }
}

struct FailingModule;

#[async_trait]
impl Module for FailingModule {
    fn input_schema(&self) -> Value {
        json!({})
    }
    fn output_schema(&self) -> Value {
        json!({})
    }
    fn description(&self) -> &str {
        "Always returns an error"
    }
    async fn execute(
        &self,
        _ctx: &Context<Value>,
        _input: Value,
    ) -> Result<Value, ModuleError> {
        Err(ModuleError::new(
            ErrorCode::GeneralInternalError,
            "intentional failure",
        ))
    }
}

fn make_ctx() -> Context<Value> {
    Context::new(Identity {
        id: "test".to_string(),
        name: "Test".to_string(),
        roles: vec![],
        attributes: HashMap::new(),
    })
}

// ---------------------------------------------------------------------------
// Module trait
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_echo_module_returns_input() {
    let ctx = make_ctx();
    let module = EchoModule;
    let input = json!({"value": "hello"});
    let result = module.execute(&ctx, input.clone()).await.unwrap();
    assert_eq!(result, input);
}

#[tokio::test]
async fn test_failing_module_returns_error() {
    let ctx = make_ctx();
    let module = FailingModule;
    let err = module.execute(&ctx, json!({})).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::GeneralInternalError);
}

#[test]
fn test_module_preflight_passes_by_default() {
    let module = EchoModule;
    let PreflightResult { passed, checks } = module.preflight();
    assert!(passed);
    assert!(checks.is_empty());
}

#[test]
fn test_module_description() {
    let module = EchoModule;
    assert_eq!(module.description(), "Echo the input value back");
}

#[test]
fn test_module_schemas_are_valid_json() {
    let module = EchoModule;
    // Both schemas must be objects
    assert!(module.input_schema().is_object());
    assert!(module.output_schema().is_object());
}

// ---------------------------------------------------------------------------
// ModuleAnnotations
// ---------------------------------------------------------------------------

#[test]
fn test_annotations_default() {
    let ann = ModuleAnnotations {
        name: "test.module".to_string(),
        ..Default::default()
    };
    assert_eq!(ann.name, "test.module");
    assert!(ann.version.is_none());
    assert!(ann.tags.is_empty());
    assert!(!ann.deprecated);
    assert!(!ann.hidden);
    assert!(ann.examples.is_empty());
}

#[test]
fn test_annotations_with_tags_and_version() {
    let ann = ModuleAnnotations {
        name: "user.get".to_string(),
        version: Some("1.0.0".to_string()),
        tags: vec!["user".to_string(), "readonly".to_string()],
        ..Default::default()
    };
    assert_eq!(ann.version.as_deref(), Some("1.0.0"));
    assert_eq!(ann.tags.len(), 2);
}

#[test]
fn test_annotations_serialization_round_trip() {
    let ann = ModuleAnnotations {
        name: "email.send".to_string(),
        description: Some("Send email".to_string()),
        tags: vec!["email".to_string()],
        deprecated: false,
        ..Default::default()
    };
    let json = serde_json::to_string(&ann).unwrap();
    let restored: ModuleAnnotations = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.name, ann.name);
    assert_eq!(restored.tags, ann.tags);
}

// ---------------------------------------------------------------------------
// ModuleExample
// ---------------------------------------------------------------------------

#[test]
fn test_module_example_fields() {
    let ex = ModuleExample {
        name: "Add two numbers".to_string(),
        description: Some("Returns the sum".to_string()),
        input: json!({"a": 1, "b": 2}),
        expected_output: json!({"result": 3}),
    };
    assert_eq!(ex.name, "Add two numbers");
    assert_eq!(ex.input["a"], 1);
    assert_eq!(ex.expected_output["result"], 3);
}

#[test]
fn test_module_example_serialization() {
    let ex = ModuleExample {
        name: "Greet Alice".to_string(),
        description: None,
        input: json!({"name": "Alice"}),
        expected_output: json!({"message": "Hello, Alice!"}),
    };
    let json = serde_json::to_string(&ex).unwrap();
    let restored: ModuleExample = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.name, ex.name);
    assert_eq!(restored.input, ex.input);
}

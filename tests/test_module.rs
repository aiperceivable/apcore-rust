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
    async fn execute(&self, input: Value, _ctx: &Context<Value>) -> Result<Value, ModuleError> {
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
    async fn execute(&self, _input: Value, _ctx: &Context<Value>) -> Result<Value, ModuleError> {
        Err(ModuleError::new(
            ErrorCode::GeneralInternalError,
            "intentional failure",
        ))
    }
}

fn make_ctx() -> Context<Value> {
    Context::new(Identity::new(
        "test".to_string(),
        "Test".to_string(),
        vec![],
        HashMap::new(),
    ))
}

// ---------------------------------------------------------------------------
// Module trait
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_echo_module_returns_input() {
    let ctx = make_ctx();
    let module = EchoModule;
    let input = json!({"value": "hello"});
    let result = module.execute(input.clone(), &ctx).await.unwrap();
    assert_eq!(result, input);
}

#[tokio::test]
async fn test_failing_module_returns_error() {
    let ctx = make_ctx();
    let module = FailingModule;
    let err = module.execute(json!({}), &ctx).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::GeneralInternalError);
}

#[test]
fn test_module_preflight_passes_by_default() {
    let module = EchoModule;
    let result = module.preflight();
    assert!(result.valid);
    assert!(result.checks.is_empty());
    assert!(!result.requires_approval);
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
        ..Default::default()
    };
    assert!(!ann.readonly);
    assert!(!ann.destructive);
    assert!(!ann.idempotent);
    assert!(!ann.requires_approval);
    assert!(ann.open_world);
    assert!(!ann.streaming);
    assert!(!ann.cacheable);
    assert_eq!(ann.cache_ttl, 0);
    assert!(ann.cache_key_fields.is_none());
    assert!(!ann.paginated);
    assert_eq!(ann.pagination_style, "cursor");
}

#[test]
fn test_annotations_with_tags_and_version() {
    let ann = ModuleAnnotations {
        readonly: true,
        idempotent: true,
        cacheable: true,
        cache_ttl: 300,
        ..Default::default()
    };
    assert!(ann.readonly);
    assert!(ann.idempotent);
    assert!(ann.cacheable);
    assert_eq!(ann.cache_ttl, 300);
}

#[test]
fn test_annotations_serialization_round_trip() {
    let ann = ModuleAnnotations {
        destructive: true,
        requires_approval: true,
        open_world: false,
        ..Default::default()
    };
    let json = serde_json::to_string(&ann).unwrap();
    let restored: ModuleAnnotations = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.destructive, ann.destructive);
    assert_eq!(restored.requires_approval, ann.requires_approval);
    assert_eq!(restored.open_world, ann.open_world);
}

// ---------------------------------------------------------------------------
// ModuleExample
// ---------------------------------------------------------------------------

#[test]
fn test_module_example_fields() {
    let ex = ModuleExample {
        title: "Add two numbers".to_string(),
        description: Some("Returns the sum".to_string()),
        inputs: json!({"a": 1, "b": 2}),
        output: json!({"result": 3}),
    };
    assert_eq!(ex.title, "Add two numbers");
    assert_eq!(ex.inputs["a"], 1);
    assert_eq!(ex.output["result"], 3);
}

#[test]
fn test_module_example_serialization() {
    let ex = ModuleExample {
        title: "Greet Alice".to_string(),
        description: None,
        inputs: json!({"name": "Alice"}),
        output: json!({"message": "Hello, Alice!"}),
    };
    let json = serde_json::to_string(&ex).unwrap();
    let restored: ModuleExample = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.title, ex.title);
    assert_eq!(restored.inputs, ex.inputs);
}

// ---------------------------------------------------------------------------
// ModuleAnnotations -- extra field (annotations-redesign)
// ---------------------------------------------------------------------------

#[test]
fn test_annotations_default_extra_is_empty() {
    let ann = ModuleAnnotations::default();
    assert!(ann.extra.is_empty());
}

#[test]
fn test_annotations_extra_with_values() {
    let mut extra = HashMap::new();
    extra.insert(
        "mcp.category".to_string(),
        serde_json::Value::String("tools".to_string()),
    );
    let ann = ModuleAnnotations {
        extra,
        ..Default::default()
    };
    assert_eq!(
        ann.extra.get("mcp.category").unwrap(),
        &serde_json::Value::String("tools".to_string())
    );
}

#[test]
fn test_annotations_serde_flatten_captures_unknown_keys() {
    // AC-028: Unknown JSON keys captured into extra via serde(flatten)
    let data = r#"{
        "readonly": true,
        "future_field": 42,
        "another_unknown": "hello"
    }"#;
    let ann: ModuleAnnotations = serde_json::from_str(data).unwrap();
    assert!(ann.readonly);
    assert_eq!(ann.extra.get("future_field").unwrap(), &json!(42));
    assert_eq!(ann.extra.get("another_unknown").unwrap(), &json!("hello"));
}

#[test]
fn test_annotations_pagination_style_accepts_custom_string() {
    let ann = ModuleAnnotations {
        pagination_style: "custom".to_string(),
        ..Default::default()
    };
    assert_eq!(ann.pagination_style, "custom");
}

#[test]
fn test_annotations_extra_round_trip() {
    let mut extra = HashMap::new();
    extra.insert("cli.approval_message".to_string(), json!("Are you sure?"));
    let ann = ModuleAnnotations {
        destructive: true,
        extra,
        ..Default::default()
    };
    let serialized = serde_json::to_string(&ann).unwrap();
    let restored: ModuleAnnotations = serde_json::from_str(&serialized).unwrap();
    assert!(restored.destructive);
    assert_eq!(
        restored.extra.get("cli.approval_message").unwrap(),
        &json!("Are you sure?")
    );
}

#[test]
fn test_annotations_deserialization_missing_fields_use_defaults() {
    let ann: ModuleAnnotations = serde_json::from_str("{}").unwrap();
    assert!(!ann.readonly);
    assert!(ann.open_world);
    assert_eq!(ann.pagination_style, "cursor");
    assert!(ann.extra.is_empty());
}

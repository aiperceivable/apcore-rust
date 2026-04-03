//! Tests for Registry — creation, read-only operations, and new methods.

use apcore::context::{Context, Identity};
use apcore::errors::ModuleError;
use apcore::module::{Module, ModuleAnnotations};
use apcore::registry::registry::{ModuleDescriptor, Registry};
use async_trait::async_trait;
use serde_json::Value;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

struct StubModule;

#[async_trait]
impl Module for StubModule {
    fn description(&self) -> &str {
        "stub"
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({ "type": "object" })
    }
    fn output_schema(&self) -> Value {
        serde_json::json!({ "type": "object" })
    }
    async fn execute(&self, _inputs: Value, _ctx: &Context<Value>) -> Result<Value, ModuleError> {
        Ok(serde_json::json!({}))
    }
}

fn make_descriptor(name: &str) -> ModuleDescriptor {
    ModuleDescriptor {
        name: name.to_string(),
        annotations: ModuleAnnotations::default(),
        input_schema: serde_json::json!({ "type": "object" }),
        output_schema: serde_json::json!({ "type": "object" }),
        enabled: true,
        tags: vec![],
        dependencies: vec![],
    }
}

fn dummy_identity() -> Identity {
    Identity::new(
        "@test".to_string(),
        "test".to_string(),
        vec![],
        Default::default(),
    )
}

// ---------------------------------------------------------------------------
// Empty-registry read tests
// ---------------------------------------------------------------------------

#[test]
fn test_registry_new_is_empty() {
    let registry = Registry::new();
    assert!(registry.list(None, None).is_empty());
}

#[test]
fn test_registry_default_is_empty() {
    let registry = Registry::default();
    assert!(registry.list(None, None).is_empty());
}

#[test]
fn test_registry_get_unknown_module_returns_none() {
    let registry = Registry::new();
    assert!(registry.get("nonexistent").is_none());
}

#[test]
fn test_registry_contains_unknown_module_returns_false() {
    let registry = Registry::new();
    assert!(!registry.has("nonexistent"));
}

#[test]
fn test_registry_get_definition_unknown_returns_none() {
    let registry = Registry::new();
    assert!(registry.get_definition("nonexistent").is_none());
}

#[test]
fn test_registry_list_returns_vec_of_str() {
    let registry = Registry::new();
    let list: Vec<&str> = registry.list(None, None);
    assert!(list.is_empty());
}

// ---------------------------------------------------------------------------
// export_schema tests (C-3)
// ---------------------------------------------------------------------------

#[test]
fn test_export_schema_returns_none_for_unregistered_module() {
    let registry = Registry::new();
    assert!(registry.export_schema("not.registered").is_none());
}

#[test]
fn test_export_schema_returns_schema_after_registration() {
    let mut registry = Registry::new();
    let descriptor = make_descriptor("math.add");
    registry
        .register_internal("math.add", Box::new(StubModule), descriptor)
        .expect("registration should succeed");

    let schema = registry.export_schema("math.add");
    assert!(
        schema.is_some(),
        "schema should be cached after registration"
    );
    let s = schema.unwrap();
    assert!(s.get("input").is_some(), "schema should have 'input' key");
    assert!(s.get("output").is_some(), "schema should have 'output' key");
}

// ---------------------------------------------------------------------------
// disable / enable / is_enabled tests (C-3)
// ---------------------------------------------------------------------------

#[test]
fn test_is_enabled_returns_none_for_unregistered_module() {
    let registry = Registry::new();
    assert!(registry.is_enabled("not.registered").is_none());
}

#[test]
fn test_disable_returns_error_for_unregistered_module() {
    let mut registry = Registry::new();
    let err = registry
        .disable("not.registered")
        .expect_err("should fail for unregistered module");
    assert!(
        err.message.contains("not found"),
        "error message should mention 'not found'"
    );
}

#[test]
fn test_enable_returns_error_for_unregistered_module() {
    let mut registry = Registry::new();
    let err = registry
        .enable("not.registered")
        .expect_err("should fail for unregistered module");
    assert!(err.message.contains("not found"));
}

#[test]
fn test_disable_sets_enabled_to_false() {
    let mut registry = Registry::new();
    registry
        .register_internal(
            "email.send",
            Box::new(StubModule),
            make_descriptor("email.send"),
        )
        .expect("registration should succeed");

    assert_eq!(registry.is_enabled("email.send"), Some(true));

    registry
        .disable("email.send")
        .expect("disable should succeed");
    assert_eq!(registry.is_enabled("email.send"), Some(false));
}

#[test]
fn test_enable_restores_enabled_to_true() {
    let mut registry = Registry::new();
    registry
        .register_internal("greet", Box::new(StubModule), make_descriptor("greet"))
        .expect("registration should succeed");

    registry.disable("greet").expect("disable should succeed");
    assert_eq!(registry.is_enabled("greet"), Some(false));

    registry.enable("greet").expect("enable should succeed");
    assert_eq!(registry.is_enabled("greet"), Some(true));
}

#[test]
fn test_module_enabled_by_default_after_registration() {
    let mut registry = Registry::new();
    registry
        .register_internal(
            "util.noop",
            Box::new(StubModule),
            make_descriptor("util.noop"),
        )
        .expect("registration should succeed");

    assert_eq!(
        registry.is_enabled("util.noop"),
        Some(true),
        "newly registered module should be enabled"
    );
}

// ---------------------------------------------------------------------------
// Reserved word validation tests
// ---------------------------------------------------------------------------

#[test]
fn test_register_rejects_reserved_first_segment() {
    let mut registry = Registry::new();
    let result = registry.register(
        "system.health",
        Box::new(StubModule),
        make_descriptor("system.health"),
    );
    assert!(result.is_err(), "registering 'system.health' should fail");
    let err = result.unwrap_err();
    let msg = format!("{}", err);
    assert!(
        msg.contains("reserved word"),
        "error should mention reserved word, got: {}",
        msg
    );
}

#[test]
fn test_register_allows_reserved_word_in_non_first_segment() {
    let mut registry = Registry::new();
    let result = registry.register(
        "email.system",
        Box::new(StubModule),
        make_descriptor("email.system"),
    );
    assert!(
        result.is_ok(),
        "registering 'email.system' should succeed — 'system' is not the first segment"
    );
}

#[test]
fn test_register_allows_normal_module_id() {
    let mut registry = Registry::new();
    let result = registry.register(
        "email.send",
        Box::new(StubModule),
        make_descriptor("email.send"),
    );
    assert!(result.is_ok(), "registering 'email.send' should succeed");
}

#[test]
fn test_register_rejects_all_reserved_words() {
    use apcore::registry::RESERVED_WORDS;
    for word in RESERVED_WORDS {
        let mut registry = Registry::new();
        let module_id = format!("{}.something", word);
        let result = registry.register(
            &module_id,
            Box::new(StubModule),
            make_descriptor(&module_id),
        );
        assert!(
            result.is_err(),
            "registering '{}' should fail — '{}' is reserved",
            module_id, word
        );
    }
}

#[test]
fn test_register_module_rejects_reserved_first_segment() {
    let mut registry = Registry::new();
    let result = registry.register_module("core.utils", Box::new(StubModule));
    assert!(
        result.is_err(),
        "register_module with 'core.utils' should fail"
    );
}

// Suppress unused-import warning — dummy_identity is available for future async tests.
#[allow(dead_code)]
fn _use_identity() -> Identity {
    dummy_identity()
}

//! Tests for Registry — creation, read-only operations, and new methods.

use apcore::context::{Context, Identity};
use apcore::errors::ModuleError;
use apcore::module::{Module, ModuleAnnotations};
use apcore::registry::registry::{ModuleDescriptor, Registry};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

struct StubModule;

#[async_trait]
impl Module for StubModule {
    fn description(&self) -> &'static str {
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
        HashMap::default(),
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
    let list: Vec<String> = registry.list(None, None);
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
    let registry = Registry::new();
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
    let registry = Registry::new();
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
    let registry = Registry::new();
    let err = registry
        .enable("not.registered")
        .expect_err("should fail for unregistered module");
    assert!(err.message.contains("not found"));
}

#[test]
fn test_disable_sets_enabled_to_false() {
    let registry = Registry::new();
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
    let registry = Registry::new();
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
    let registry = Registry::new();
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
    let registry = Registry::new();
    let result = registry.register(
        "system.health",
        Box::new(StubModule),
        make_descriptor("system.health"),
    );
    assert!(result.is_err(), "registering 'system.health' should fail");
    let err = result.unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("reserved word"),
        "error should mention reserved word, got: {msg}"
    );
}

#[test]
fn test_register_rejects_reserved_word_in_any_segment() {
    // PROTOCOL_SPEC §2.7: reserved words MUST NOT appear as ANY segment of a
    // module ID (not just the first). Aligned with apcore-python and
    // apcore-typescript, both of which reject 'email.system' for this reason.
    let registry = Registry::new();
    let result = registry.register(
        "email.system",
        Box::new(StubModule),
        make_descriptor("email.system"),
    );
    assert!(
        result.is_err(),
        "registering 'email.system' must fail — 'system' is reserved in any segment"
    );
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.contains("reserved word") && msg.contains("system"),
        "error should mention reserved word 'system', got: {msg}"
    );
}

#[test]
fn test_register_allows_normal_module_id() {
    let registry = Registry::new();
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
        let registry = Registry::new();
        let module_id = format!("{word}.something");
        let result = registry.register(
            &module_id,
            Box::new(StubModule),
            make_descriptor(&module_id),
        );
        assert!(
            result.is_err(),
            "registering '{module_id}' should fail — '{word}' is reserved"
        );
    }
}

#[test]
fn test_register_module_rejects_reserved_first_segment() {
    let registry = Registry::new();
    let result = registry.register_module("core.utils", Box::new(StubModule));
    assert!(
        result.is_err(),
        "register_module with 'core.utils' should fail"
    );
}

// ---------------------------------------------------------------------------
// Module ID length boundary tests (PROTOCOL_SPEC §2.7 EBNF constraint #1)
// ---------------------------------------------------------------------------

#[test]
fn test_max_module_id_length_matches_spec() {
    // Per PROTOCOL_SPEC §2.7. Bumped from 128 to 192 in spec 1.6.0-draft.
    // Filesystem-safe: 192 + ".binding.yaml".len()=13 = 205 < 255-byte filename limit.
    use apcore::registry::MAX_MODULE_ID_LENGTH;
    assert_eq!(MAX_MODULE_ID_LENGTH, 192);
}

#[test]
fn test_register_accepts_module_id_at_max_length() {
    use apcore::registry::MAX_MODULE_ID_LENGTH;
    let registry = Registry::new();
    // Pure 'a' run satisfies the EBNF pattern [a-z][a-z0-9_]*.
    let exact_id = "a".repeat(MAX_MODULE_ID_LENGTH);
    let result = registry.register(&exact_id, Box::new(StubModule), make_descriptor(&exact_id));
    assert!(
        result.is_ok(),
        "registering an ID at exactly MAX_MODULE_ID_LENGTH should succeed"
    );
}

#[test]
fn test_register_rejects_module_id_exceeding_max_length() {
    use apcore::registry::MAX_MODULE_ID_LENGTH;
    let registry = Registry::new();
    let overlong_id = "a".repeat(MAX_MODULE_ID_LENGTH + 1);
    let result = registry.register(
        &overlong_id,
        Box::new(StubModule),
        make_descriptor(&overlong_id),
    );
    assert!(
        result.is_err(),
        "registering an ID longer than MAX_MODULE_ID_LENGTH should fail"
    );
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.contains("maximum length"),
        "error should mention maximum length, got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// PROTOCOL_SPEC §2.7 EBNF compliance — empty / pattern checks
// (parity with apcore-python and apcore-typescript)
// ---------------------------------------------------------------------------

#[test]
fn test_register_rejects_empty_module_id() {
    let registry = Registry::new();
    let result = registry.register("", Box::new(StubModule), make_descriptor(""));
    assert!(result.is_err(), "registering empty ID must fail");
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.contains("non-empty"),
        "error should mention non-empty, got: {msg}"
    );
}

#[test]
fn test_register_rejects_invalid_pattern() {
    let registry = Registry::new();
    for bad_id in [
        "INVALID-ID", // hyphens not allowed
        "1abc",       // starts with digit
        "Module",     // uppercase
        "a..b",       // consecutive dots
        ".leading",   // leading dot
        "trailing.",  // trailing dot
        "has space",  // space
        "has!bang",   // special char
    ] {
        let result = registry.register(bad_id, Box::new(StubModule), make_descriptor(bad_id));
        assert!(
            result.is_err(),
            "registering pattern-invalid ID '{bad_id}' must fail"
        );
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("Invalid module ID") || msg.contains("Must match pattern"),
            "error for '{bad_id}' should mention pattern, got: {msg}"
        );
    }
}

// ---------------------------------------------------------------------------
// register_internal — bypasses ONLY reserved word check
// (parity with apcore-python and apcore-typescript)
// ---------------------------------------------------------------------------

#[test]
fn test_register_internal_accepts_reserved_first_segment() {
    let registry = Registry::new();
    let result = registry.register_internal(
        "system.health",
        Box::new(StubModule),
        make_descriptor("system.health"),
    );
    assert!(
        result.is_ok(),
        "register_internal must accept reserved first segment 'system'"
    );
}

#[test]
fn test_register_internal_accepts_reserved_any_segment() {
    let registry = Registry::new();
    let result = registry.register_internal(
        "myapp.system.config",
        Box::new(StubModule),
        make_descriptor("myapp.system.config"),
    );
    assert!(
        result.is_ok(),
        "register_internal must accept reserved word in any segment"
    );
}

#[test]
fn test_register_internal_still_rejects_empty() {
    let registry = Registry::new();
    let result = registry.register_internal("", Box::new(StubModule), make_descriptor(""));
    assert!(
        result.is_err(),
        "register_internal must still reject empty IDs"
    );
}

#[test]
fn test_register_internal_still_rejects_invalid_pattern() {
    let registry = Registry::new();
    let result = registry.register_internal(
        "INVALID-ID",
        Box::new(StubModule),
        make_descriptor("INVALID-ID"),
    );
    assert!(
        result.is_err(),
        "register_internal must still enforce EBNF pattern"
    );
}

#[test]
fn test_register_internal_still_rejects_over_length() {
    use apcore::registry::MAX_MODULE_ID_LENGTH;
    let registry = Registry::new();
    let overlong = "a".repeat(MAX_MODULE_ID_LENGTH + 1);
    let result =
        registry.register_internal(&overlong, Box::new(StubModule), make_descriptor(&overlong));
    assert!(
        result.is_err(),
        "register_internal must still enforce length limit"
    );
}

#[test]
fn test_register_internal_rejects_duplicate() {
    let registry = Registry::new();
    registry
        .register_internal(
            "system.dup",
            Box::new(StubModule),
            make_descriptor("system.dup"),
        )
        .expect("first register_internal should succeed");
    let result = registry.register_internal(
        "system.dup",
        Box::new(StubModule),
        make_descriptor("system.dup"),
    );
    assert!(
        result.is_err(),
        "register_internal must reject duplicate IDs"
    );
}

// Suppress unused-import warning — dummy_identity is available for future async tests.
#[allow(dead_code)]
fn _use_identity() -> Identity {
    dummy_identity()
}

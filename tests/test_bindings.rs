//! Integration tests for BindingLoader with Registry and FunctionModule.

use apcore::bindings::{BindingDefinition, BindingHandler, BindingLoader, BindingTarget};
use apcore::context::Context;
use apcore::errors::ModuleError;
use apcore::registry::registry::Registry;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_echo_handler() -> BindingHandler {
    Arc::new(|inputs: Value, _ctx: &Context<Value>| Box::pin(async move { Ok(inputs) }))
}

fn make_error_handler() -> BindingHandler {
    Arc::new(|_inputs: Value, _ctx: &Context<Value>| {
        Box::pin(async move {
            Err(ModuleError::new(
                apcore::errors::ErrorCode::GeneralInternalError,
                "handler intentionally failed".to_string(),
            ))
        })
    })
}

fn binding_def(name: &str, module_name: &str) -> BindingDefinition {
    BindingDefinition {
        name: name.to_string(),
        target: BindingTarget {
            module_name: module_name.to_string(),
            callable: "handler".to_string(),
            schema_path: None,
        },
        metadata: HashMap::new(),
    }
}

// ---------------------------------------------------------------------------
// register_into_with_handlers
// ---------------------------------------------------------------------------

#[test]
fn register_single_binding_into_registry() {
    let registry = Registry::new();
    let mut loader = BindingLoader::new();

    // Manually insert a definition (bypasses file I/O)
    let def = binding_def("echo_binding", "test.echo");
    let mut inner = HashMap::new();
    inner.insert("echo_binding".to_string(), def);

    // Load via a temp JSON file so we exercise the public API.
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("bindings.json");
    std::fs::write(
        &file_path,
        serde_json::to_string(&json!([
            {"name": "echo_binding", "target": {"module_name": "test.echo", "callable": "h"}, "metadata": {}}
        ]))
        .unwrap(),
    )
    .unwrap();
    loader.load_from_file(&file_path).unwrap();

    let mut handlers: HashMap<String, BindingHandler> = HashMap::new();
    handlers.insert("echo_binding".to_string(), make_echo_handler());

    let count = loader
        .register_into_with_handlers(&registry, handlers)
        .unwrap();

    assert_eq!(count, 1);
    assert!(registry.get("test.echo").is_some());
}

#[test]
fn register_multiple_bindings_into_registry() {
    let registry = Registry::new();
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("bindings.json");
    std::fs::write(
        &file_path,
        serde_json::to_string(&json!([
            {"name": "b1", "target": {"module_name": "test.b1", "callable": "h"}, "metadata": {}},
            {"name": "b2", "target": {"module_name": "test.b2", "callable": "h"}, "metadata": {}},
            {"name": "b3", "target": {"module_name": "test.b3", "callable": "h"}, "metadata": {}}
        ]))
        .unwrap(),
    )
    .unwrap();

    let mut loader = BindingLoader::new();
    loader.load_from_file(&file_path).unwrap();

    let mut handlers: HashMap<String, BindingHandler> = HashMap::new();
    for name in ["b1", "b2", "b3"] {
        handlers.insert(name.to_string(), make_echo_handler());
    }

    let count = loader
        .register_into_with_handlers(&registry, handlers)
        .unwrap();
    assert_eq!(count, 3);
    assert!(registry.get("test.b1").is_some());
    assert!(registry.get("test.b2").is_some());
    assert!(registry.get("test.b3").is_some());
}

#[test]
fn register_binding_with_description_in_metadata() {
    let registry = Registry::new();
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("bindings.json");
    std::fs::write(
        &file_path,
        serde_json::to_string(&json!([
            {
                "name": "described",
                "target": {"module_name": "test.described", "callable": "h"},
                "metadata": {"description": "A well-described module"}
            }
        ]))
        .unwrap(),
    )
    .unwrap();

    let mut loader = BindingLoader::new();
    loader.load_from_file(&file_path).unwrap();

    let mut handlers: HashMap<String, BindingHandler> = HashMap::new();
    handlers.insert("described".to_string(), make_echo_handler());

    let count = loader
        .register_into_with_handlers(&registry, handlers)
        .unwrap();
    assert_eq!(count, 1);
    assert!(registry.get("test.described").is_some());
}

#[test]
fn register_fails_when_handler_missing_for_binding() {
    let registry = Registry::new();
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("bindings.json");
    std::fs::write(
        &file_path,
        serde_json::to_string(&json!([
            {"name": "needs_handler", "target": {"module_name": "test.x", "callable": "h"}, "metadata": {}}
        ]))
        .unwrap(),
    )
    .unwrap();

    let mut loader = BindingLoader::new();
    loader.load_from_file(&file_path).unwrap();

    // Provide an empty handler map — should fail.
    let result = loader.register_into_with_handlers(&registry, HashMap::new());
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code, apcore::errors::ErrorCode::BindingModuleNotFound);
    assert!(err.message.contains("needs_handler"));
}

#[test]
fn register_empty_loader_succeeds_with_zero_count() {
    let registry = Registry::new();
    let loader = BindingLoader::new();
    let count = loader
        .register_into_with_handlers(&registry, HashMap::new())
        .unwrap();
    assert_eq!(count, 0);
}

// ---------------------------------------------------------------------------
// End-to-end: load from YAML dir, register, execute via Registry
// ---------------------------------------------------------------------------

#[tokio::test]
async fn end_to_end_yaml_load_and_registry_execution() {
    let dir = tempfile::tempdir().unwrap();
    let yaml = r"
- name: yaml_echo
  target:
    module_name: test.yaml_echo
    callable: echo_fn
  metadata:
    description: Echo inputs back as output
";
    std::fs::write(dir.path().join("echo.binding.yaml"), yaml).unwrap();

    let mut loader = BindingLoader::new();
    let n_loaded = loader.load_binding_dir(dir.path(), None).unwrap();
    assert_eq!(n_loaded, 1);

    let registry = Registry::new();
    let mut handlers: HashMap<String, BindingHandler> = HashMap::new();
    handlers.insert("yaml_echo".to_string(), make_echo_handler());

    let registered = loader
        .register_into_with_handlers(&registry, handlers)
        .unwrap();
    assert_eq!(registered, 1);

    // Call the registered module through the Registry.
    let module = registry
        .get("test.yaml_echo")
        .expect("module should be registered");
    let identity = apcore::context::Identity::new(
        "tester".to_string(),
        "service".to_string(),
        vec![],
        HashMap::new(),
    );
    let ctx: Context<Value> = Context::new(identity);
    let inputs = json!({"key": "value"});
    let output = module.execute(inputs.clone(), &ctx).await.unwrap();
    assert_eq!(output, inputs);
}

#[tokio::test]
async fn registered_handler_propagates_error() {
    let dir = tempfile::tempdir().unwrap();
    let yaml = r"
- name: failing_binding
  target:
    module_name: test.failing
    callable: fail_fn
  metadata: {}
";
    std::fs::write(dir.path().join("fail.binding.yaml"), yaml).unwrap();

    let mut loader = BindingLoader::new();
    loader.load_binding_dir(dir.path(), None).unwrap();

    let registry = Registry::new();
    let mut handlers: HashMap<String, BindingHandler> = HashMap::new();
    handlers.insert("failing_binding".to_string(), make_error_handler());

    loader
        .register_into_with_handlers(&registry, handlers)
        .unwrap();

    let module = registry
        .get("test.failing")
        .expect("module should be registered");
    let identity = apcore::context::Identity::new(
        "tester".to_string(),
        "service".to_string(),
        vec![],
        HashMap::new(),
    );
    let ctx: Context<Value> = Context::new(identity);
    let result = module.execute(json!({}), &ctx).await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .message
        .contains("handler intentionally failed"));
}

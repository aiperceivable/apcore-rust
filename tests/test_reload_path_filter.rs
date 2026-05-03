//! Issue #45.4 — granular reload via `path_filter`.
//!
//! Verifies that ReloadModule with `path_filter` only re-discovers / unregisters
//! modules whose IDs match the glob, leaving others intact.

use std::collections::HashMap;
use std::sync::Arc;

use apcore::context::{Context, Identity};
use apcore::events::emitter::EventEmitter;
use apcore::module::{Module, ModuleAnnotations};
use apcore::registry::registry::{ModuleDescriptor, Registry};
use apcore::sys_modules::control::ReloadModule;
use tokio::sync::Mutex;

fn dummy_ctx() -> Context<serde_json::Value> {
    Context::<serde_json::Value>::new(Identity::new(
        "@test".to_string(),
        "test".to_string(),
        vec![],
        HashMap::default(),
    ))
}

fn register_dummy(registry: &Arc<Registry>, id: &str) {
    struct Dummy;
    #[async_trait::async_trait]
    impl Module for Dummy {
        fn description(&self) -> &'static str {
            "dummy"
        }
        fn input_schema(&self) -> serde_json::Value {
            serde_json::json!({})
        }
        fn output_schema(&self) -> serde_json::Value {
            serde_json::json!({})
        }
        async fn execute(
            &self,
            _i: serde_json::Value,
            _c: &Context<serde_json::Value>,
        ) -> Result<serde_json::Value, apcore::errors::ModuleError> {
            Ok(serde_json::json!({}))
        }
    }
    let descriptor = ModuleDescriptor {
        module_id: id.to_string(),
        name: None,
        description: String::new(),
        documentation: None,
        input_schema: serde_json::json!({}),
        output_schema: serde_json::json!({}),
        version: "1.0.0".to_string(),
        tags: vec![],
        annotations: Some(ModuleAnnotations::default()),
        examples: vec![],
        metadata: std::collections::HashMap::new(),
        display: None,
        sunset_date: None,
        dependencies: vec![],
        enabled: true,
    };
    registry
        .register_internal(id, Box::new(Dummy), descriptor)
        .expect("register_internal");
}

#[tokio::test]
async fn path_filter_only_reloads_matching_modules() {
    let registry = Arc::new(Registry::new());
    let emitter = Arc::new(Mutex::new(EventEmitter::new()));

    register_dummy(&registry, "executor.email.send");
    register_dummy(&registry, "executor.email.recv");
    register_dummy(&registry, "executor.sms.send");
    register_dummy(&registry, "common.helpers.format");

    let reload = ReloadModule::new(Arc::clone(&registry), Arc::clone(&emitter));
    let result = reload
        .execute(
            serde_json::json!({
                "path_filter": "executor.email.*",
                "reason": "granular reload",
            }),
            &dummy_ctx(),
        )
        .await
        .expect("path_filter reload should succeed");

    let reloaded = result["reloaded_modules"]
        .as_array()
        .expect("reloaded_modules array");
    let reloaded_ids: Vec<String> = reloaded
        .iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect();

    // Only executor.email.* should be reloaded.
    assert!(
        reloaded_ids.contains(&"executor.email.send".to_string()),
        "executor.email.send should be in reloaded list, got {reloaded_ids:?}"
    );
    assert!(
        reloaded_ids.contains(&"executor.email.recv".to_string()),
        "executor.email.recv should be in reloaded list, got {reloaded_ids:?}"
    );
    assert!(
        !reloaded_ids.contains(&"executor.sms.send".to_string()),
        "executor.sms.send must NOT match executor.email.*, got {reloaded_ids:?}"
    );
    assert!(
        !reloaded_ids.contains(&"common.helpers.format".to_string()),
        "common.helpers.format must NOT match, got {reloaded_ids:?}"
    );

    // Non-matching modules remain registered.
    assert!(registry.has("executor.sms.send"));
    assert!(registry.has("common.helpers.format"));
}

#[tokio::test]
async fn module_id_and_path_filter_are_mutually_exclusive() {
    let registry = Arc::new(Registry::new());
    let emitter = Arc::new(Mutex::new(EventEmitter::new()));
    let reload = ReloadModule::new(registry, emitter);
    let err = reload
        .execute(
            serde_json::json!({
                "module_id": "executor.email.send",
                "path_filter": "executor.*",
                "reason": "should fail",
            }),
            &dummy_ctx(),
        )
        .await
        .expect_err("should error on conflict");
    assert_eq!(
        err.code,
        apcore::errors::ErrorCode::ModuleReloadConflict,
        "expected MODULE_RELOAD_CONFLICT"
    );
}

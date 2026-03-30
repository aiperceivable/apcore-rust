//! Integration tests for sys_modules control modules.

use std::sync::Arc;

use apcore::config::Config;
use apcore::context::{Context, Identity};
use apcore::errors::ErrorCode;
use apcore::events::emitter::EventEmitter;
use apcore::executor::Executor;
use apcore::module::{Module, ModuleAnnotations};
use apcore::registry::registry::{ModuleDescriptor, Registry};
use apcore::sys_modules::{
    check_module_disabled, is_module_disabled, register_sys_modules, ToggleFeatureModule,
    ToggleState, UpdateConfigModule,
};
use tokio::sync::Mutex;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_config() -> Arc<Mutex<Config>> {
    Arc::new(Mutex::new(Config::default()))
}

fn make_emitter() -> Arc<Mutex<EventEmitter>> {
    Arc::new(Mutex::new(EventEmitter::new()))
}

fn make_registry() -> Arc<Mutex<Registry>> {
    Arc::new(Mutex::new(Registry::new()))
}

fn dummy_ctx() -> Context<serde_json::Value> {
    Context::<serde_json::Value>::new(Identity {
        id: "@test".to_string(),
        identity_type: "test".to_string(),
        roles: vec![],
        attrs: Default::default(),
    })
}

async fn register_dummy(registry: &Arc<Mutex<Registry>>, id: &str) {
    struct DummyModule;
    #[async_trait::async_trait]
    impl Module for DummyModule {
        fn description(&self) -> &str {
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
            _inputs: serde_json::Value,
            _ctx: &Context<serde_json::Value>,
        ) -> Result<serde_json::Value, apcore::errors::ModuleError> {
            Ok(serde_json::json!({}))
        }
    }

    let descriptor = ModuleDescriptor {
        name: id.to_string(),
        annotations: ModuleAnnotations::default(),
        input_schema: serde_json::json!({}),
        output_schema: serde_json::json!({}),
        enabled: true,
        tags: vec![],
        dependencies: vec![],
    };
    registry
        .lock()
        .await
        .register_internal(id, Box::new(DummyModule), descriptor)
        .expect("register_internal should succeed");
}

// ---------------------------------------------------------------------------
// ToggleState tests
// ---------------------------------------------------------------------------

#[test]
fn test_toggle_state_new_has_nothing_disabled() {
    let ts = ToggleState::new();
    assert!(!ts.is_disabled("my_module"));
}

#[test]
fn test_toggle_state_disable_marks_module() {
    let ts = ToggleState::new();
    ts.disable("mod_a");
    assert!(ts.is_disabled("mod_a"));
    assert!(!ts.is_disabled("mod_b"));
}

#[test]
fn test_toggle_state_enable_removes_module() {
    let ts = ToggleState::new();
    ts.disable("mod_a");
    ts.enable("mod_a");
    assert!(!ts.is_disabled("mod_a"));
}

#[test]
fn test_toggle_state_clear_empties_set() {
    let ts = ToggleState::new();
    ts.disable("mod_a");
    ts.disable("mod_b");
    ts.clear();
    assert!(!ts.is_disabled("mod_a"));
    assert!(!ts.is_disabled("mod_b"));
}

#[test]
fn test_is_module_disabled_and_check_module_disabled() {
    // The global toggle state persists across tests in the same process, so
    // use a unique ID to avoid interference from other tests.
    let unique_id = "test_global_toggle_unique_12345";
    assert!(!is_module_disabled(unique_id));
    assert!(check_module_disabled(unique_id).is_ok());
}

// ---------------------------------------------------------------------------
// UpdateConfigModule tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_update_config_module_returns_correct_result() {
    let config = make_config();
    let emitter = make_emitter();
    let module = UpdateConfigModule::new(config, emitter);
    let ctx = dummy_ctx();

    let inputs = serde_json::json!({
        "key": "max_call_depth",
        "value": 64,
        "reason": "increase depth for testing"
    });

    let result = module
        .execute(inputs, &ctx)
        .await
        .expect("execute should succeed");

    assert_eq!(result["success"], serde_json::json!(true));
    assert_eq!(result["key"], serde_json::json!("max_call_depth"));
    assert_eq!(result["new_value"], serde_json::json!(64));
}

#[tokio::test]
async fn test_update_config_module_missing_key_returns_error() {
    let config = make_config();
    let emitter = make_emitter();
    let module = UpdateConfigModule::new(config, emitter);
    let ctx = dummy_ctx();

    let inputs = serde_json::json!({
        "value": 64,
        "reason": "no key provided"
    });

    let err = module
        .execute(inputs, &ctx)
        .await
        .expect_err("should fail on missing key");
    assert!(err.message.contains("'key'"));
}

#[tokio::test]
async fn test_update_config_module_missing_reason_returns_error() {
    let config = make_config();
    let emitter = make_emitter();
    let module = UpdateConfigModule::new(config, emitter);
    let ctx = dummy_ctx();

    let inputs = serde_json::json!({
        "key": "max_call_depth",
        "value": 64
    });

    let err = module
        .execute(inputs, &ctx)
        .await
        .expect_err("should fail on missing reason");
    assert!(err.message.contains("'reason'"));
}

#[tokio::test]
async fn test_update_config_module_restricted_key_returns_error() {
    let config = make_config();
    let emitter = make_emitter();
    let module = UpdateConfigModule::new(config, emitter);
    let ctx = dummy_ctx();

    let inputs = serde_json::json!({
        "key": "sys_modules.enabled",
        "value": false,
        "reason": "trying to disable sys modules"
    });

    let err = module
        .execute(inputs, &ctx)
        .await
        .expect_err("should reject restricted key");
    assert!(err.message.contains("cannot be changed at runtime"));
}

// ---------------------------------------------------------------------------
// ToggleFeatureModule tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_toggle_feature_module_disables_and_enables() {
    let registry = make_registry();
    register_dummy(&registry, "my.module").await;

    let emitter = make_emitter();
    let toggle_state = Arc::new(ToggleState::new());
    let module =
        ToggleFeatureModule::new(Arc::clone(&registry), emitter, Arc::clone(&toggle_state));
    let ctx = dummy_ctx();

    // Disable
    let result = module
        .execute(
            serde_json::json!({
                "module_id": "my.module",
                "enabled": false,
                "reason": "testing disable"
            }),
            &ctx,
        )
        .await
        .expect("disable should succeed");

    assert_eq!(result["success"], serde_json::json!(true));
    assert_eq!(result["module_id"], serde_json::json!("my.module"));
    assert_eq!(result["enabled"], serde_json::json!(false));
    assert!(toggle_state.is_disabled("my.module"));

    // Re-enable
    let result = module
        .execute(
            serde_json::json!({
                "module_id": "my.module",
                "enabled": true,
                "reason": "testing enable"
            }),
            &ctx,
        )
        .await
        .expect("enable should succeed");

    assert_eq!(result["enabled"], serde_json::json!(true));
    assert!(!toggle_state.is_disabled("my.module"));
}

#[tokio::test]
async fn test_toggle_feature_module_not_found_returns_error() {
    let registry = make_registry();
    let emitter = make_emitter();
    let toggle_state = Arc::new(ToggleState::new());
    let module = ToggleFeatureModule::new(registry, emitter, toggle_state);
    let ctx = dummy_ctx();

    let err = module
        .execute(
            serde_json::json!({
                "module_id": "nonexistent.module",
                "enabled": false,
                "reason": "should not matter"
            }),
            &ctx,
        )
        .await
        .expect_err("should fail when module not in registry");

    assert_eq!(err.code, ErrorCode::ModuleNotFound);
}

// ---------------------------------------------------------------------------
// register_sys_modules integration tests (C-3)
// ---------------------------------------------------------------------------

#[test]
fn test_register_sys_modules_returns_none_when_disabled() {
    let registry = Arc::new(Mutex::new(Registry::new()));
    let mut config = Config::default();
    config.set("sys_modules.enabled", serde_json::json!(false));
    let mut executor = Executor::new(Registry::new(), Config::default());

    let result = register_sys_modules(Arc::clone(&registry), &mut executor, &config, None);
    assert!(
        result.is_none(),
        "should return None when sys_modules.enabled=false"
    );
}

#[test]
fn test_register_sys_modules_registers_control_modules_into_caller_registry() {
    // register_sys_modules is synchronous and uses blocking_lock internally;
    // call it outside any tokio runtime, then use block_on only for async assertions.
    let registry = Arc::new(Mutex::new(Registry::new()));
    let mut config = Config::default();
    config.set("sys_modules.enabled", serde_json::json!(true));
    let mut executor = Executor::new(Registry::new(), Config::default());

    let ctx = register_sys_modules(Arc::clone(&registry), &mut executor, &config, None);
    assert!(
        ctx.is_some(),
        "should return Some when sys_modules.enabled=true"
    );

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build");
    rt.block_on(async {
        let reg = registry.lock().await;
        assert!(
            reg.has("system.control.update_config"),
            "update_config should be in caller's registry"
        );
        assert!(
            reg.has("system.control.reload_module"),
            "reload_module should be in caller's registry"
        );
        assert!(
            reg.has("system.control.toggle_feature"),
            "toggle_feature should be in caller's registry"
        );
    });
}

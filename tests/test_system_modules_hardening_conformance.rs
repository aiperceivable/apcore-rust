//! Cross-language conformance tests for System Modules Hardening (Issue #45).
//!
//! Fixture source: apcore/conformance/fixtures/system_modules_hardening.json
//! Spec reference: apcore/docs/features/system-modules.md (## System Modules Hardening)
//!
//! Each fixture case verifies one normative rule of the hardening surface:
//! overrides persistence, contextual audit trail, Prometheus UsageCollector
//! exporter, path-filter reload (with mutual exclusion), and the breaking
//! `register_sys_modules` signature change in Rust.

#![allow(clippy::missing_panics_doc)]
#![allow(clippy::cast_possible_truncation)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};

use apcore::config::Config;
use apcore::context::{Context, Identity};
use apcore::errors::ErrorCode;
use apcore::events::emitter::EventEmitter;
use apcore::executor::Executor;
use apcore::module::Module;
use apcore::observability::usage::UsageCollector;
use apcore::registry::registry::Registry;
use apcore::sys_modules::audit::{AuditAction, AuditStore, InMemoryAuditStore};
use apcore::sys_modules::control::{ReloadModule, ToggleFeatureModule, UpdateConfigModule};
use apcore::sys_modules::overrides::load_overrides;
use apcore::sys_modules::{
    register_sys_modules, register_sys_modules_with_options, SysModulesOptions, ToggleState,
};
use tokio::sync::Mutex;

// ---------------------------------------------------------------------------
// Fixture loading (mirrors other conformance tests)
// ---------------------------------------------------------------------------

fn find_fixtures_root() -> PathBuf {
    if let Ok(spec_repo) = std::env::var("APCORE_SPEC_REPO") {
        let p = PathBuf::from(&spec_repo)
            .join("conformance")
            .join("fixtures");
        if p.is_dir() {
            return p;
        }
        panic!("APCORE_SPEC_REPO={spec_repo} does not contain conformance/fixtures/");
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let sibling = manifest_dir
        .parent()
        .unwrap()
        .join("apcore")
        .join("conformance")
        .join("fixtures");
    if sibling.is_dir() {
        return sibling;
    }

    panic!(
        "Cannot find apcore conformance fixtures.\n\
         Fix one of:\n\
         1. Set APCORE_SPEC_REPO to the apcore spec repo path\n\
         2. Clone apcore as a sibling: git clone <apcore-url> {}\n",
        manifest_dir.parent().unwrap().join("apcore").display()
    );
}

fn load_fixture() -> Value {
    let path = find_fixtures_root().join("system_modules_hardening.json");
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("Failed to read fixture: {}", path.display()));
    serde_json::from_str(&content).unwrap_or_else(|e| panic!("Invalid JSON: {e}"))
}

fn fixture_case<'a>(fixture: &'a Value, id: &str) -> &'a Value {
    fixture["test_cases"]
        .as_array()
        .expect("test_cases must be an array")
        .iter()
        .find(|c| c["id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("fixture case '{id}' not present"))
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

fn make_ctx(id: Option<(&str, &str)>) -> Context<serde_json::Value> {
    Context {
        trace_id: "trace-test".to_string(),
        identity: id
            .map(|(i, t)| Identity::new(i.to_string(), t.to_string(), vec![], HashMap::new())),
        services: serde_json::Value::Null,
        caller_id: None,
        data: Arc::new(parking_lot::RwLock::new(HashMap::new())),
        call_chain: vec![],
        redacted_inputs: None,
        redacted_output: None,
        cancel_token: None,
        global_deadline: None,
        executor: None,
    }
}

/// Unique tempfile path for tests. Avoids collisions when cases run in parallel.
fn temp_overrides_path(label: &str) -> PathBuf {
    let pid = std::process::id();
    let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
    std::env::temp_dir().join(format!("apcore_overrides_{label}_{pid}_{nanos}.yaml"))
}

// ---------------------------------------------------------------------------
// §1.1 Config and Feature Toggle Persistence
// ---------------------------------------------------------------------------

#[tokio::test]
async fn case_overrides_persisted_on_update() {
    let fixture = load_fixture();
    let _case = fixture_case(&fixture, "overrides_persisted_on_update");

    let path = temp_overrides_path("persist");
    let _ = std::fs::remove_file(&path);

    let config = Config::default();
    let config_arc = Arc::new(Mutex::new(config));
    let emitter = Arc::new(Mutex::new(EventEmitter::new()));

    let module = UpdateConfigModule::new(Arc::clone(&config_arc), Arc::clone(&emitter))
        .with_overrides_path(Some(path.clone()));

    let inputs = json!({
        "key": "executor.default_timeout",
        "value": 60000,
        "reason": "increase timeout for tests",
    });
    let ctx = make_ctx(None);
    let out = module
        .execute(inputs, &ctx)
        .await
        .expect("call should succeed");
    assert_eq!(out["success"], json!(true));

    assert!(path.exists(), "overrides file should be written");
    let raw = std::fs::read_to_string(&path).expect("overrides readable");
    let parsed: serde_yaml_ng::Value = serde_yaml_ng::from_str(&raw).expect("valid YAML");
    let map = parsed.as_mapping().expect("top-level mapping");
    let v = map
        .get(serde_yaml_ng::Value::String(
            "executor.default_timeout".to_string(),
        ))
        .expect("key persisted");
    assert_eq!(v.as_i64(), Some(60000));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn case_overrides_loaded_on_startup() {
    let fixture = load_fixture();
    let _case = fixture_case(&fixture, "overrides_loaded_on_startup");

    let path = temp_overrides_path("startup");
    std::fs::write(&path, "executor.default_timeout: 60000\n").unwrap();

    let mut config = Config::default();
    config.set("executor.default_timeout", json!(30000));

    load_overrides(&path, &mut config, None);

    let resolved = config
        .get("executor.default_timeout")
        .expect("key resolved")
        .as_i64()
        .unwrap();
    assert_eq!(resolved, 60000, "override value must win over base");

    let _ = std::fs::remove_file(&path);
}

// ---------------------------------------------------------------------------
// §1.2 Contextual Audit Trail
// ---------------------------------------------------------------------------

#[tokio::test]
async fn case_audit_entry_records_actor() {
    let fixture = load_fixture();
    let _case = fixture_case(&fixture, "audit_entry_records_actor");

    let inspect = Arc::new(InMemoryAuditStore::new());
    let store: Arc<dyn AuditStore> = inspect.clone();

    let config = Arc::new(Mutex::new(Config::default()));
    let emitter = Arc::new(Mutex::new(EventEmitter::new()));
    let module = UpdateConfigModule::new(Arc::clone(&config), Arc::clone(&emitter))
        .with_audit_store(Some(Arc::clone(&store)));

    let inputs = json!({
        "key": "executor.default_timeout",
        "value": 45000,
        "reason": "audit trail test",
    });
    let ctx = make_ctx(Some(("user-abc-123", "user")));
    module
        .execute(inputs, &ctx)
        .await
        .expect("call should succeed");

    let entries = inspect.entries();
    assert_eq!(entries.len(), 1, "exactly one audit entry expected");
    let e = &entries[0];
    assert_eq!(e.action, AuditAction::UpdateConfig);
    assert_eq!(e.target_module_id, "system.control.update_config");
    assert_eq!(e.actor_id, "user-abc-123");
    assert_eq!(e.actor_type, "user");
    assert!(!e.trace_id.is_empty(), "trace_id must be present");
    // timestamp is required and chrono::DateTime is always present in the struct
}

#[tokio::test]
async fn case_audit_entry_records_change() {
    let fixture = load_fixture();
    let _case = fixture_case(&fixture, "audit_entry_records_change");

    let inspect = Arc::new(InMemoryAuditStore::new());
    let store: Arc<dyn AuditStore> = inspect.clone();

    // Seed a target module so the toggle call passes the registry check.
    let registry = Arc::new(Registry::new());
    register_dummy_module(&registry, "risky.module");

    let emitter = Arc::new(Mutex::new(EventEmitter::new()));
    let toggle_state = Arc::new(ToggleState::new());
    let module = ToggleFeatureModule::new(
        Arc::clone(&registry),
        Arc::clone(&emitter),
        Arc::clone(&toggle_state),
    )
    .with_audit_store(Some(Arc::clone(&store)));

    let inputs = json!({
        "module_id": "risky.module",
        "enabled": false,
        "reason": "maintenance window",
    });
    let ctx = make_ctx(Some(("svc-deploy-agent", "service")));
    module
        .execute(inputs, &ctx)
        .await
        .expect("call should succeed");

    let entries = inspect.entries();
    assert_eq!(entries.len(), 1);
    let e = &entries[0];
    assert_eq!(e.action, AuditAction::ToggleFeature);
    assert_eq!(e.target_module_id, "risky.module");
    assert_eq!(e.actor_id, "svc-deploy-agent");
    assert_eq!(e.actor_type, "service");
    assert_eq!(e.change.before, json!(true));
    assert_eq!(e.change.after, json!(false));
}

// ---------------------------------------------------------------------------
// §1.3 Prometheus exporter for UsageCollector
// ---------------------------------------------------------------------------

#[test]
fn case_prometheus_usage_exports_calls_total() {
    let fixture = load_fixture();
    let _case = fixture_case(&fixture, "prometheus_usage_exports_calls_total");

    let collector = UsageCollector::new();
    // Seed: 4998 success + 2 error for math.add. We bound the test to the
    // ratios that matter (success vs error) — emitting all 5000 records would
    // be wasteful and the export is independent of total count beyond the
    // success/error split.
    for _ in 0..4998 {
        collector.record("math.add", None, 12.0, true);
    }
    for _ in 0..2 {
        collector.record("math.add", None, 12.0, false);
    }

    let body = collector.export_prometheus();
    let required_lines = [
        "apcore_usage_calls_total{module_id=\"math.add\",status=\"success\"}",
        "apcore_usage_calls_total{module_id=\"math.add\",status=\"error\"}",
        "apcore_usage_error_rate{module_id=\"math.add\"}",
        "apcore_usage_p99_latency_ms{module_id=\"math.add\"}",
    ];
    for line in required_lines {
        assert!(
            body.contains(line),
            "Prometheus export missing {line}\n--- body ---\n{body}"
        );
    }
}

// ---------------------------------------------------------------------------
// §1.4 Granular reload via path filtering
// ---------------------------------------------------------------------------

#[tokio::test]
async fn case_reload_with_path_filter() {
    let fixture = load_fixture();
    let _case = fixture_case(&fixture, "reload_with_path_filter");

    let registry = Arc::new(Registry::new());
    register_dummy_module(&registry, "executor.email.send");
    register_dummy_module(&registry, "executor.math.add");
    register_dummy_module(&registry, "executor.pdf.render");
    register_dummy_module(&registry, "orchestrator.main");

    let emitter = Arc::new(Mutex::new(EventEmitter::new()));
    let module = ReloadModule::new(Arc::clone(&registry), emitter);

    let inputs = json!({
        "path_filter": "executor.*",
        "reload_dependents": false,
        "reason": "bulk reload after deploy",
    });
    let ctx = make_ctx(None);
    let out = module
        .execute(inputs, &ctx)
        .await
        .expect("bulk reload should succeed");

    assert_eq!(out["success"], json!(true));
    let reloaded: Vec<String> = out["reloaded_modules"]
        .as_array()
        .expect("reloaded_modules array")
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();

    let expected: std::collections::HashSet<&str> = [
        "executor.email.send",
        "executor.math.add",
        "executor.pdf.render",
    ]
    .into_iter()
    .collect();
    let actual: std::collections::HashSet<&str> = reloaded.iter().map(String::as_str).collect();
    assert_eq!(actual, expected, "all matching modules must be reloaded");
    assert!(
        !reloaded.iter().any(|m| m == "orchestrator.main"),
        "non-matching module must be skipped"
    );
}

#[tokio::test]
async fn case_reload_module_id_and_filter_conflict() {
    let fixture = load_fixture();
    let _case = fixture_case(&fixture, "reload_module_id_and_filter_conflict");

    let registry = Arc::new(Registry::new());
    let emitter = Arc::new(Mutex::new(EventEmitter::new()));
    let module = ReloadModule::new(Arc::clone(&registry), emitter);

    let inputs = json!({
        "module_id": "executor.email.send",
        "path_filter": "executor.*",
        "reason": "conflict test",
    });
    let ctx = make_ctx(None);
    let err = module
        .execute(inputs, &ctx)
        .await
        .expect_err("conflict should raise");

    assert_eq!(
        err.code,
        ErrorCode::ModuleReloadConflict,
        "expected MODULE_RELOAD_CONFLICT"
    );
    assert!(
        err.message.contains("mutually exclusive"),
        "error message must explain the conflict, got: {}",
        err.message
    );
}

// ---------------------------------------------------------------------------
// §1.5 Startup failure handling (Rust-specific Result signature)
// ---------------------------------------------------------------------------

#[test]
fn case_rust_register_returns_result() {
    let fixture = load_fixture();
    let _case = fixture_case(&fixture, "rust_register_returns_result");

    // Successful registration: returns Ok(SysModulesContext).
    let registry = Arc::new(Registry::new());
    let mut config = Config::default();
    config.set("sys_modules.enabled", json!(true));
    let executor = Executor::new(Arc::clone(&registry), Config::default());
    let result = register_sys_modules(Arc::clone(&registry), &executor, &config, None);
    assert!(
        result.is_ok(),
        "successful registration must return Ok(SysModulesContext)"
    );
    let ctx = result.unwrap();
    assert!(!ctx.registered_modules.is_empty(), "must register modules");
}

#[test]
fn case_startup_fail_on_error_true_raises() {
    let fixture = load_fixture();
    let _case = fixture_case(&fixture, "startup_fail_on_error_true_raises");

    // Pre-register a sys module so the second registration attempt raises
    // ModuleAlreadyRegistered, which fail_on_error=true must surface as
    // SysModuleError::RegistrationFailed.
    let registry = Arc::new(Registry::new());
    register_dummy_module(&registry, "system.health.summary");

    let mut config = Config::default();
    config.set("sys_modules.enabled", json!(true));
    let executor = Executor::new(Arc::clone(&registry), Config::default());

    let result = register_sys_modules_with_options(
        Arc::clone(&registry),
        &executor,
        &config,
        None,
        SysModulesOptions {
            fail_on_error: true,
            ..Default::default()
        },
    );
    let Err(err) = result else {
        panic!("fail_on_error=true must propagate")
    };
    assert_eq!(err.module_id(), "system.health.summary");
    assert_eq!(err.error_code(), ErrorCode::SysModuleRegistrationFailed);
}

#[test]
fn case_startup_fail_on_error_false_continues() {
    let fixture = load_fixture();
    let _case = fixture_case(&fixture, "startup_fail_on_error_false_continues");

    // Same setup as the strict case, but fail_on_error=false must swallow
    // the error and let the remaining modules register.
    let registry = Arc::new(Registry::new());
    register_dummy_module(&registry, "system.health.summary");

    let mut config = Config::default();
    config.set("sys_modules.enabled", json!(true));
    let executor = Executor::new(Arc::clone(&registry), Config::default());

    let result = register_sys_modules_with_options(
        Arc::clone(&registry),
        &executor,
        &config,
        None,
        SysModulesOptions::default(),
    );
    let ctx = result.expect("fail_on_error=false must succeed");
    assert!(
        registry.has("system.manifest.full"),
        "remaining modules must still register after a failure"
    );
    // The pre-existing dummy under `system.health.summary` blocks the sys
    // module from registering; the sys module is therefore absent from the
    // returned `registered_modules` map.
    assert!(
        !ctx.registered_modules.contains_key("system.health.summary"),
        "the failed module must not appear in registered_modules"
    );
}

// ---------------------------------------------------------------------------
// Regression: code-review fixes
// ---------------------------------------------------------------------------

/// Issue #45 review fix #1 (D1 finding 1):
/// `UpdateConfigModule` must redact `old_value`/`new_value` on sensitive keys
/// in (a) the response payload and (b) the `AuditChange.before/after` so an
/// external `AuditStore` does not receive plaintext secrets. Mirrors Python
/// reference impl `apcore-python/src/apcore/sys_modules/control.py:220-236`.
#[tokio::test]
async fn regression_update_config_redacts_sensitive_keys() {
    let inspect = Arc::new(InMemoryAuditStore::new());
    let store: Arc<dyn AuditStore> = inspect.clone();

    let mut base_config = Config::default();
    base_config.set("auth.api_key", json!("OLD_SECRET"));
    let config = Arc::new(Mutex::new(base_config));
    let emitter = Arc::new(Mutex::new(EventEmitter::new()));

    let module = UpdateConfigModule::new(Arc::clone(&config), Arc::clone(&emitter))
        .with_audit_store(Some(Arc::clone(&store)));

    let inputs = json!({
        "key": "auth.api_key",
        "value": "NEW_SECRET",
        "reason": "rotate credential",
    });
    let ctx = make_ctx(Some(("user-rotator", "user")));
    let out = module
        .execute(inputs, &ctx)
        .await
        .expect("update should succeed");

    // (a) Response payload must not leak either value.
    assert_eq!(out["old_value"], json!("***REDACTED***"));
    assert_eq!(out["new_value"], json!("***REDACTED***"));
    let raw = serde_json::to_string(&out).unwrap();
    assert!(
        !raw.contains("OLD_SECRET") && !raw.contains("NEW_SECRET"),
        "raw secret must not appear in response payload: {raw}"
    );

    // (b) AuditEntry must carry redacted before/after.
    let entries = inspect.entries();
    assert_eq!(entries.len(), 1);
    let e = &entries[0];
    assert_eq!(e.change.before, json!("***REDACTED***"));
    assert_eq!(e.change.after, json!("***REDACTED***"));

    // (c) The in-memory Config must still hold the real new value — redaction
    // is for egress only, not for runtime state.
    let stored = config.lock().await.get("auth.api_key");
    assert_eq!(stored, Some(json!("NEW_SECRET")));
}

/// Same redaction MUST NOT apply to non-sensitive keys.
#[tokio::test]
async fn regression_update_config_does_not_redact_normal_keys() {
    let inspect = Arc::new(InMemoryAuditStore::new());
    let store: Arc<dyn AuditStore> = inspect.clone();

    let mut base_config = Config::default();
    base_config.set("executor.default_timeout", json!(30000));
    let config = Arc::new(Mutex::new(base_config));
    let emitter = Arc::new(Mutex::new(EventEmitter::new()));

    let module = UpdateConfigModule::new(Arc::clone(&config), Arc::clone(&emitter))
        .with_audit_store(Some(Arc::clone(&store)));

    let ctx = make_ctx(Some(("user-1", "user")));
    let out = module
        .execute(
            json!({"key":"executor.default_timeout","value":60000,"reason":"tune"}),
            &ctx,
        )
        .await
        .unwrap();
    assert_eq!(out["old_value"], json!(30000));
    assert_eq!(out["new_value"], json!(60000));

    let entries = inspect.entries();
    assert_eq!(entries[0].change.before, json!(30000));
    assert_eq!(entries[0].change.after, json!(60000));
}

/// Issue #45 review fix #2 (D1 finding 2):
/// When `events.enabled=false` AND a caller sets `audit_store` or
/// `overrides_path` on `SysModulesOptions`, control modules are not registered
/// — so the options are silent no-ops. The function must emit a `WARN`-level
/// tracing event so the misconfiguration is observable.
#[test]
fn regression_options_warn_when_events_disabled() {
    use std::sync::{Arc as StdArc, Mutex as StdMutex};

    #[derive(Clone, Default)]
    struct CapturedLogs(StdArc<StdMutex<Vec<u8>>>);

    impl std::io::Write for CapturedLogs {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedLogs {
        type Writer = CapturedLogs;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    let captured = CapturedLogs::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(captured.clone())
        .with_max_level(tracing::Level::WARN)
        .with_ansi(false)
        .with_target(false)
        .finish();

    let registry = Arc::new(Registry::new());
    let mut config = Config::default();
    config.set("sys_modules.enabled", json!(true));
    config.set("sys_modules.events.enabled", json!(false));
    let executor = Executor::new(Arc::clone(&registry), Config::default());

    let store: Arc<dyn AuditStore> = Arc::new(InMemoryAuditStore::new());

    let result = tracing::subscriber::with_default(subscriber, || {
        register_sys_modules_with_options(
            Arc::clone(&registry),
            &executor,
            &config,
            None,
            SysModulesOptions {
                overrides_path: Some(PathBuf::from("/tmp/should_not_be_written.yaml")),
                audit_store: Some(store),
                fail_on_error: false,
            },
        )
    });
    assert!(result.is_ok());

    let logs = String::from_utf8_lossy(&captured.0.lock().unwrap()).into_owned();
    assert!(
        logs.contains("events.enabled=false") || logs.contains("have no effect"),
        "expected WARN about disabled events to mention the no-effect condition, got: {logs}"
    );
    assert!(
        logs.to_uppercase().contains("WARN"),
        "expected WARN level event, got: {logs}"
    );
}

/// When events are ENABLED, no warning fires (control flow path is the
/// happy path; the warning is opt-in misconfiguration detection).
#[test]
fn regression_no_warn_when_events_enabled() {
    use std::sync::{Arc as StdArc, Mutex as StdMutex};

    #[derive(Clone, Default)]
    struct CapturedLogs(StdArc<StdMutex<Vec<u8>>>);

    impl std::io::Write for CapturedLogs {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedLogs {
        type Writer = CapturedLogs;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    let captured = CapturedLogs::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(captured.clone())
        .with_max_level(tracing::Level::WARN)
        .with_ansi(false)
        .with_target(false)
        .finish();

    let registry = Arc::new(Registry::new());
    let mut config = Config::default();
    config.set("sys_modules.enabled", json!(true));
    config.set("sys_modules.events.enabled", json!(true));
    let executor = Executor::new(Arc::clone(&registry), Config::default());

    let store: Arc<dyn AuditStore> = Arc::new(InMemoryAuditStore::new());

    let _ = tracing::subscriber::with_default(subscriber, || {
        register_sys_modules_with_options(
            Arc::clone(&registry),
            &executor,
            &config,
            None,
            SysModulesOptions {
                overrides_path: None,
                audit_store: Some(store),
                fail_on_error: false,
            },
        )
        .expect("should succeed")
    });

    let logs = String::from_utf8_lossy(&captured.0.lock().unwrap()).into_owned();
    assert!(
        !logs.contains("have no effect"),
        "expected NO no-effect warning when events are enabled, got: {logs}"
    );
}

// ---------------------------------------------------------------------------
// Test fixtures: minimal Module impl used as a placeholder for registry seeding
// ---------------------------------------------------------------------------

fn register_dummy_module(registry: &Arc<Registry>, module_id: &str) {
    use apcore::registry::registry::ModuleDescriptor;
    let module: Box<dyn Module> = Box::new(DummyModule);
    let descriptor = ModuleDescriptor {
        module_id: module_id.to_string(),
        name: None,
        description: "test module".to_string(),
        documentation: None,
        input_schema: json!({"type": "object"}),
        output_schema: json!({"type": "object"}),
        version: "1.0.0".to_string(),
        tags: vec![],
        annotations: None,
        examples: vec![],
        metadata: HashMap::new(),
        display: None,
        sunset_date: None,
        dependencies: vec![],
        enabled: true,
    };
    registry
        .register_internal(module_id, module, descriptor)
        .expect("dummy registration");
}

struct DummyModule;

#[async_trait::async_trait]
impl Module for DummyModule {
    fn description(&self) -> &'static str {
        "dummy"
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }
    fn output_schema(&self) -> Value {
        json!({"type": "object"})
    }
    async fn execute(
        &self,
        _inputs: Value,
        _ctx: &Context<Value>,
    ) -> Result<Value, apcore::errors::ModuleError> {
        Ok(json!({}))
    }
}

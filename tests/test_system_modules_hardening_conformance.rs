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
    register_sys_modules, register_sys_modules_with_options, SysModuleError, SysModulesContext,
    SysModulesOptions, ToggleState,
};
use tokio::sync::Mutex;

// ---------------------------------------------------------------------------
// Fixture loading (mirrors other conformance tests)
// ---------------------------------------------------------------------------

use crate::conformance_env::find_fixtures_root;

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

/// In-memory `tracing` writer so tests can assert on emitted log records.
#[derive(Clone, Default)]
struct CapturedLogs(Arc<std::sync::Mutex<Vec<u8>>>);

impl CapturedLogs {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

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
    let case = fixture_case(&fixture, "overrides_persisted_on_update");
    let expected = &case["expected"];

    let path = temp_overrides_path("persist");
    let _ = std::fs::remove_file(&path);

    let config = Config::default();
    let config_arc = Arc::new(Mutex::new(config));
    let emitter = Arc::new(EventEmitter::new());

    let module = UpdateConfigModule::new(Arc::clone(&config_arc), Arc::clone(&emitter))
        .with_overrides_path(Some(path.clone()));

    // Inputs come from the fixture so the persisted key/value below is the one
    // the contract names, not a copy that can silently drift from it.
    let inputs = case["action"]["input"].clone();
    let ctx = make_ctx(None);
    let out = module
        .execute(inputs, &ctx)
        .await
        .expect("call should succeed");

    // `call_success`
    assert_eq!(
        out["success"].as_bool(),
        expected["call_success"].as_bool(),
        "call_success mismatch"
    );

    // `overrides_file_written`
    assert_eq!(
        path.exists(),
        expected["overrides_file_written"].as_bool().unwrap(),
        "overrides_file_written mismatch for {}",
        path.display()
    );

    // `overrides_file_contains` — every declared key/value must be present in
    // the YAML the SDK actually wrote.
    let raw = std::fs::read_to_string(&path).expect("overrides readable");
    let parsed: serde_yaml_ng::Value = serde_yaml_ng::from_str(&raw).expect("valid YAML");
    let map = parsed.as_mapping().expect("top-level mapping");
    for (key, want) in expected["overrides_file_contains"]
        .as_object()
        .expect("overrides_file_contains is an object")
    {
        let got = map
            .get(serde_yaml_ng::Value::String(key.clone()))
            .unwrap_or_else(|| panic!("overrides file is missing key {key}; file:\n{raw}"));
        let got_json: Value = serde_yaml_ng::from_value(got.clone()).expect("YAML value to JSON");
        assert_eq!(&got_json, want, "overrides_file_contains[{key}] mismatch");
    }

    let _ = std::fs::remove_file(&path);
}

#[test]
fn case_overrides_loaded_on_startup() {
    let fixture = load_fixture();
    let case = fixture_case(&fixture, "overrides_loaded_on_startup");
    let expected = &case["expected"];

    // The base config is written to a real file so `base_not_modified` has
    // something observable to be true OF: an implementation that folded the
    // override back into its source would rewrite this file.
    let base_path = temp_overrides_path("startup_base");
    // `Config::from_yaml_file` deserializes into the typed Config tree, so the
    // fixture's flat dotted keys are expanded into nested YAML mappings.
    let mut base_tree = serde_json::Map::new();
    // `Config::validate()` requires these two regardless of what the case
    // exercises; they are scaffolding, not part of the contract under test.
    base_tree.insert("version".to_string(), json!("1.0"));
    base_tree.insert("project".to_string(), json!({"name": "conformance"}));
    for (key, value) in case["setup"]["base_config"]
        .as_object()
        .expect("setup.base_config is an object")
    {
        let mut cursor = &mut base_tree;
        let segments: Vec<&str> = key.split('.').collect();
        for segment in &segments[..segments.len() - 1] {
            cursor = cursor
                .entry((*segment).to_string())
                .or_insert_with(|| Value::Object(serde_json::Map::new()))
                .as_object_mut()
                .expect("nested mapping");
        }
        cursor.insert(segments[segments.len() - 1].to_string(), value.clone());
    }
    let base_yaml =
        serde_yaml_ng::to_string(&Value::Object(base_tree)).expect("base config serializes");
    std::fs::write(&base_path, &base_yaml).unwrap();
    let base_bytes_before = std::fs::read(&base_path).unwrap();

    let path = temp_overrides_path("startup");
    let mut overrides_yaml = String::new();
    for (key, value) in case["setup"]["overrides_file_content"]
        .as_object()
        .expect("setup.overrides_file_content is an object")
    {
        overrides_yaml.push_str(&format!("{key}: {value}\n"));
    }
    std::fs::write(&path, &overrides_yaml).unwrap();

    let mut config = Config::from_yaml_file(&base_path).expect("base config loads");

    load_overrides(&path, &mut config, None);

    // `resolved_value` — the override must win over the base.
    let want_key = expected["resolved_value"]["key"].as_str().unwrap();
    let resolved = config.get(want_key).expect("key resolved");
    assert_eq!(
        resolved, expected["resolved_value"]["value"],
        "resolved_value for {want_key} mismatch"
    );

    // `base_not_modified` — loading overrides MUST NOT write back to the base
    // config source. Checked both byte-wise and by re-reading the base through
    // the loader, so a rewrite that happens to be byte-different or
    // semantically different is caught either way.
    let base_unmodified = std::fs::read(&base_path).unwrap() == base_bytes_before
        && Config::from_yaml_file(&base_path)
            .expect("base config still loads")
            .get(want_key)
            == case["setup"]["base_config"].get(want_key).cloned();
    assert_eq!(
        base_unmodified,
        expected["base_not_modified"].as_bool().unwrap(),
        "base_not_modified mismatch — the base config source at {} changed",
        base_path.display()
    );

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&base_path);
}

// ---------------------------------------------------------------------------
// §1.2 Contextual Audit Trail
// ---------------------------------------------------------------------------

#[tokio::test]
async fn case_audit_entry_records_actor() {
    let fixture = load_fixture();
    let case = fixture_case(&fixture, "audit_entry_records_actor");
    let expected = &case["expected"];

    let inspect = Arc::new(InMemoryAuditStore::new());
    let store: Arc<dyn AuditStore> = inspect.clone();

    let config = Arc::new(Mutex::new(Config::default()));
    let emitter = Arc::new(EventEmitter::new());
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

    // `audit_entries_count`
    assert_eq!(
        entries.len() as u64,
        expected["audit_entries_count"].as_u64().unwrap(),
        "audit_entries_count mismatch"
    );

    // `audit_entry` — compare the SDK's serialized entry field-for-field
    // against the fixture's declared entry, so a renamed/dropped field or a
    // wrong actor is caught by the same assertion that reads the key.
    let entry_json = serde_json::to_value(&entries[0]).expect("AuditEntry serializes");
    assert_audit_entry_matches(&entry_json, &expected["audit_entry"]);

    // `timestamp_present` — the entry must actually carry a parsable RFC 3339
    // timestamp on the wire, not merely have the field in the Rust struct.
    let has_timestamp = entry_json
        .get("timestamp")
        .and_then(Value::as_str)
        .is_some_and(|t| chrono::DateTime::parse_from_rfc3339(t).is_ok());
    assert_eq!(
        has_timestamp,
        expected["timestamp_present"].as_bool().unwrap(),
        "timestamp_present mismatch; entry: {entry_json}"
    );

    // `trace_id_present`
    let has_trace_id = entry_json
        .get("trace_id")
        .and_then(Value::as_str)
        .is_some_and(|t| !t.is_empty());
    assert_eq!(
        has_trace_id,
        expected["trace_id_present"].as_bool().unwrap(),
        "trace_id_present mismatch; entry: {entry_json}"
    );
    // Belt-and-braces on the typed side: the enum variant behind the wire name.
    assert_eq!(entries[0].action, AuditAction::UpdateConfig);
}

/// Assert every field the fixture declares on `audit_entry` is present with
/// that value in the SDK's serialized `AuditEntry`. Nested objects (`change`)
/// are compared recursively; fields the fixture does not mention are ignored.
fn assert_audit_entry_matches(actual: &Value, want: &Value) {
    for (field, want_value) in want.as_object().expect("audit_entry is an object") {
        let got = actual
            .get(field)
            .unwrap_or_else(|| panic!("audit entry is missing field '{field}'; got {actual}"));
        if want_value.is_object() {
            assert_audit_entry_matches(got, want_value);
        } else {
            assert_eq!(got, want_value, "audit_entry.{field} mismatch in {actual}");
        }
    }
}

#[tokio::test]
async fn case_audit_entry_records_change() {
    let fixture = load_fixture();
    let case = fixture_case(&fixture, "audit_entry_records_change");
    let expected = &case["expected"];

    let inspect = Arc::new(InMemoryAuditStore::new());
    let store: Arc<dyn AuditStore> = inspect.clone();

    // Seed a target module so the toggle call passes the registry check.
    let registry = Arc::new(Registry::new());
    register_dummy_module(&registry, "risky.module");

    let emitter = Arc::new(EventEmitter::new());
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

    // `audit_entries_count`
    assert_eq!(
        entries.len() as u64,
        expected["audit_entries_count"].as_u64().unwrap(),
        "audit_entries_count mismatch"
    );

    // `audit_entry` — including the nested `change.before` / `change.after`.
    let entry_json = serde_json::to_value(&entries[0]).expect("AuditEntry serializes");
    assert_audit_entry_matches(&entry_json, &expected["audit_entry"]);
    assert_eq!(entries[0].action, AuditAction::ToggleFeature);
}

// ---------------------------------------------------------------------------
// §1.3 Prometheus exporter for UsageCollector
// ---------------------------------------------------------------------------

#[test]
fn case_prometheus_usage_exports_calls_total() {
    let fixture = load_fixture();
    let case = fixture_case(&fixture, "prometheus_usage_exports_calls_total");
    let expected = &case["expected"];

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

    // `export_within_timeout_ms` — the export is the thing being timed, so the
    // clock brackets exactly the call under test.
    let started = std::time::Instant::now();
    let body = collector.export_prometheus();
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    assert!(
        elapsed_ms < expected["export_within_timeout_ms"].as_u64().unwrap(),
        "export_prometheus took {elapsed_ms}ms, over the fixture's budget of {}ms",
        expected["export_within_timeout_ms"]
    );

    // `metrics_endpoint_contains` — every series the fixture names.
    for line in expected["metrics_endpoint_contains"]
        .as_array()
        .expect("metrics_endpoint_contains is an array")
    {
        let line = line.as_str().expect("series name is a string");
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
    let case = fixture_case(&fixture, "reload_with_path_filter");
    let expected = &case["expected"];

    let registry = Arc::new(Registry::new());
    for module_id in case["setup"]["registered_modules"]
        .as_array()
        .expect("setup.registered_modules is an array")
    {
        register_dummy_module(&registry, module_id.as_str().unwrap());
    }

    let emitter = Arc::new(EventEmitter::new());
    let module = ReloadModule::new(Arc::clone(&registry), emitter);

    let ctx = make_ctx(None);
    let out = module
        .execute(case["action"]["input"].clone(), &ctx)
        .await
        .expect("bulk reload should succeed");

    // `call_success`
    assert_eq!(
        out["success"].as_bool(),
        expected["call_success"].as_bool(),
        "call_success mismatch"
    );

    let reloaded: Vec<&str> = out["reloaded_modules"]
        .as_array()
        .expect("reloaded_modules array")
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();

    // `reloaded_modules` is compared IN ORDER: the sequence is the observable
    // consequence of `reload_order` below, so an unordered set comparison would
    // discard the very thing the fixture names.
    let want_reloaded: Vec<&str> = expected["reloaded_modules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        reloaded, want_reloaded,
        "reloaded_modules must match the fixture in order"
    );

    // `not_reloaded` — modules outside the path filter.
    for skipped in expected["not_reloaded"]
        .as_array()
        .expect("not_reloaded is an array")
    {
        let skipped = skipped.as_str().unwrap();
        assert!(
            !reloaded.contains(&skipped),
            "{skipped} is declared not_reloaded but appears in {reloaded:?}"
        );
    }

    // `reload_order: "topological"` — the fixture's own modules declare no
    // dependencies, so their order alone cannot distinguish topological from
    // alphabetical. Re-run the same bulk reload over a graph that DOES have an
    // edge: `executor.email.send` depends on `executor.pdf.render`, which must
    // therefore be reloaded first (leaves first) even though it sorts later
    // alphabetically. That inversion is only produced by a topological sort.
    assert_eq!(
        expected["reload_order"].as_str().unwrap(),
        "topological",
        "this case only knows how to verify a topological reload_order"
    );
    let dep_registry = Arc::new(Registry::new());
    register_dummy_module(&dep_registry, "executor.pdf.render");
    register_dummy_module_with_dep(&dep_registry, "executor.email.send", "executor.pdf.render");
    let dep_module = ReloadModule::new(Arc::clone(&dep_registry), Arc::new(EventEmitter::new()));
    let dep_out = dep_module
        .execute(case["action"]["input"].clone(), &make_ctx(None))
        .await
        .expect("bulk reload should succeed");
    let dep_order: Vec<&str> = dep_out["reloaded_modules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        dep_order,
        vec!["executor.pdf.render", "executor.email.send"],
        "reload_order must be topological (dependency first), not alphabetical"
    );
}

#[tokio::test]
async fn case_reload_module_id_and_filter_conflict() {
    let fixture = load_fixture();
    let _case = fixture_case(&fixture, "reload_module_id_and_filter_conflict");

    let registry = Arc::new(Registry::new());
    let emitter = Arc::new(EventEmitter::new());
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

/// Report the `T` and `E` of a `Result<T, E>` by the compiler's own name for
/// them. Nothing in this file names either type: the values come from the real
/// return type of the function under test, so changing that signature changes
/// what these assertions see. Applying it at all requires the value to BE a
/// `Result` — an `Option` or a bare value does not type-check here.
fn result_type_names<T, E>(_: &Result<T, E>) -> (&'static str, &'static str) {
    (std::any::type_name::<T>(), std::any::type_name::<E>())
}

/// Last `::`-separated segment of a fully-qualified type path.
/// `apcore::sys_modules::SysModuleError` -> `SysModuleError`; `()` -> `()`.
fn short_type_name(path: &str) -> &str {
    path.rsplit("::").next().unwrap_or(path)
}

/// Compile-time pin of the `register_sys_modules` signature.
///
/// This coercion type-checks only while the function takes exactly these four
/// parameters and returns exactly `Result<SysModulesContext, SysModuleError>`.
/// Adding an `executor`-style parameter, switching the return to `Option`, or
/// changing either `Result` arm breaks the BUILD — before any assertion runs.
/// The runtime checks in `case_rust_register_returns_result` then compare the
/// same signature, observed reflectively, against the fixture's declared shape.
const _REGISTER_SYS_MODULES_SIGNATURE: fn(
    Arc<Registry>,
    &Executor,
    &Config,
    Option<apcore::observability::MetricsCollector>,
) -> Result<SysModulesContext, SysModuleError> = register_sys_modules;

#[test]
fn case_rust_register_returns_result() {
    let fixture = load_fixture();
    let case = fixture_case(&fixture, "rust_register_returns_result");
    let expected = &case["expected"];

    let registry = Arc::new(Registry::new());
    let mut config = Config::default();
    config.set("sys_modules.enabled", json!(true));
    let executor = Executor::new(Arc::clone(&registry), Config::default());

    // `panics: false` — observed, not assumed. A `register_sys_modules` that
    // unwrapped instead of returning Err would surface here as Err(payload).
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        register_sys_modules(Arc::clone(&registry), &executor, &config, None)
    }));
    assert_eq!(
        outcome.is_err(),
        expected["panics"].as_bool().expect("panics is a bool"),
        "register_sys_modules panic behaviour disagrees with the fixture"
    );
    let result = outcome.expect("register_sys_modules must not panic");

    // `return_type` / `returns_option` — read off the real return type via
    // `type_name_of_val`, e.g. "core::result::Result<..., ...>". The head
    // segment is the constructor the SDK actually returns.
    let full_type = std::any::type_name_of_val(&result);
    let head = short_type_name(full_type.split('<').next().unwrap_or(full_type));
    assert_eq!(
        head,
        expected["return_type"]
            .as_str()
            .expect("return_type is a string"),
        "register_sys_modules returns {full_type}, which is not the fixture's declared return_type"
    );
    assert_eq!(
        head == "Option",
        expected["returns_option"]
            .as_bool()
            .expect("returns_option is a bool"),
        "register_sys_modules returns {full_type}"
    );

    // `ok_variant` / `err_variant` — the Ok and Err arms of the real signature.
    let (ok_type, err_type) = result_type_names(&result);
    assert_eq!(
        short_type_name(ok_type),
        expected["ok_variant"]
            .as_str()
            .expect("ok_variant is a string"),
        "Ok arm of register_sys_modules is {ok_type}"
    );
    assert_eq!(
        short_type_name(err_type),
        expected["err_variant"]
            .as_str()
            .expect("err_variant is a string"),
        "Err arm of register_sys_modules is {err_type}"
    );

    // The success path still has to do its job.
    let ctx = result.expect("successful registration must return Ok");
    assert!(!ctx.registered_modules.is_empty(), "must register modules");
}

#[test]
fn case_startup_fail_on_error_true_raises() {
    let fixture = load_fixture();
    let case = fixture_case(&fixture, "startup_fail_on_error_true_raises");
    let expected = &case["expected"];
    let failing_module_id = case["setup"]["simulated_failure"]["module_id"]
        .as_str()
        .expect("setup declares the failing module");

    // Pre-register a sys module so the second registration attempt raises
    // ModuleAlreadyRegistered, which fail_on_error=true must surface as
    // SysModuleError::RegistrationFailed.
    let registry = Arc::new(Registry::new());
    register_dummy_module(&registry, failing_module_id);

    let mut config = Config::default();
    config.set("sys_modules.enabled", json!(true));
    let executor = Executor::new(Arc::clone(&registry), Config::default());

    let result = register_sys_modules_with_options(
        Arc::clone(&registry),
        &executor,
        &config,
        None,
        SysModulesOptions {
            fail_on_error: case["action"]["params"]["fail_on_error"]
                .as_bool()
                .expect("action.params.fail_on_error"),
            ..Default::default()
        },
    );

    // `raises`
    assert_eq!(
        result.is_err(),
        expected["raises"].as_bool().unwrap(),
        "raises mismatch"
    );
    let Err(err) = result else {
        panic!("fail_on_error=true must propagate")
    };

    // `error_includes_module_id` — the error must name the module that failed
    // so callers can route recovery per module.
    assert_eq!(
        err.module_id(),
        expected["error_includes_module_id"].as_str().unwrap(),
        "error_includes_module_id mismatch; error was: {err}"
    );
    assert!(
        err.to_string()
            .contains(expected["error_includes_module_id"].as_str().unwrap()),
        "the rendered error message must also carry the module_id: {err}"
    );

    // `error_code`
    assert_eq!(
        serde_json::to_value(err.error_code()).unwrap(),
        expected["error_code"],
        "error_code mismatch"
    );
    assert_eq!(err.error_code(), ErrorCode::SysModuleRegistrationFailed);
}

#[test]
fn case_startup_fail_on_error_false_continues() {
    let fixture = load_fixture();
    let case = fixture_case(&fixture, "startup_fail_on_error_false_continues");
    let expected = &case["expected"];
    let failing_module_id = case["setup"]["simulated_failure"]["module_id"]
        .as_str()
        .expect("setup declares the failing module");

    // Same setup as the strict case, but fail_on_error=false must swallow
    // the error and let the remaining modules register.
    let registry = Arc::new(Registry::new());
    register_dummy_module(&registry, failing_module_id);

    let mut config = Config::default();
    config.set("sys_modules.enabled", json!(true));
    let executor = Executor::new(Arc::clone(&registry), Config::default());

    // `log_level_on_failure` — the swallowed failure MUST still be visible in
    // the logs at the declared level, otherwise a lenient startup is silent.
    // The registration runs synchronously on this thread, so a scoped
    // subscriber captures it.
    let captured = CapturedLogs::default();
    let level = expected["log_level_on_failure"]
        .as_str()
        .expect("log_level_on_failure is a string");
    let subscriber = tracing_subscriber::fmt()
        .with_writer(captured.clone())
        .with_max_level(tracing_level_from_name(level))
        .with_ansi(false)
        .with_target(false)
        .finish();

    let result = tracing::subscriber::with_default(subscriber, || {
        register_sys_modules_with_options(
            Arc::clone(&registry),
            &executor,
            &config,
            None,
            SysModulesOptions {
                fail_on_error: case["action"]["params"]["fail_on_error"]
                    .as_bool()
                    .expect("action.params.fail_on_error"),
                ..Default::default()
            },
        )
    });

    // `raises`
    assert_eq!(
        result.is_err(),
        expected["raises"].as_bool().unwrap(),
        "raises mismatch"
    );
    let ctx = result.expect("fail_on_error=false must succeed");

    let logs = captured.text();
    let failure_lines: Vec<&str> = logs
        .lines()
        .filter(|l| l.contains(failing_module_id))
        .collect();
    assert!(
        !failure_lines.is_empty(),
        "no log line mentions the failed module {failing_module_id}; captured:\n{logs}"
    );
    assert!(
        failure_lines.iter().all(|l| l.contains(level)),
        "the registration failure must be logged at {level}; captured:\n{logs}"
    );

    // `remaining_modules_registered` — a failure on one module must not stop
    // the rest. `system.manifest.full` is a sibling sys module registered after
    // the failing one.
    let remaining_registered = registry.has("system.manifest.full");
    assert_eq!(
        remaining_registered,
        expected["remaining_modules_registered"].as_bool().unwrap(),
        "remaining_modules_registered mismatch"
    );
    // The pre-existing dummy under the failing id blocks the sys module from
    // registering; the sys module is therefore absent from the returned
    // `registered_modules` map.
    assert!(
        !ctx.registered_modules.contains_key(failing_module_id),
        "the failed module must not appear in registered_modules"
    );
}

/// Map the fixture's log-level name onto a `tracing` level.
fn tracing_level_from_name(name: &str) -> tracing::Level {
    match name {
        "ERROR" => tracing::Level::ERROR,
        "WARN" => tracing::Level::WARN,
        "INFO" => tracing::Level::INFO,
        "DEBUG" => tracing::Level::DEBUG,
        "TRACE" => tracing::Level::TRACE,
        other => panic!("fixture declares an unknown log level: {other}"),
    }
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
    let emitter = Arc::new(EventEmitter::new());

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
    let emitter = Arc::new(EventEmitter::new());

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
                overrides_store: None,
                audit_store: Some(store),
                fail_on_error: false,
                ..Default::default()
            },
        )
    });
    assert!(result.is_ok());

    let logs = captured.text();
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
                overrides_store: None,
                audit_store: Some(store),
                fail_on_error: false,
                ..Default::default()
            },
        )
        .expect("should succeed")
    });

    let logs = captured.text();
    assert!(
        !logs.contains("have no effect"),
        "expected NO no-effect warning when events are enabled, got: {logs}"
    );
}

// ---------------------------------------------------------------------------
// Test fixtures: minimal Module impl used as a placeholder for registry seeding
// ---------------------------------------------------------------------------

/// Register a dummy module that declares a hard dependency on `depends_on`.
/// Used to give the topological reload check a real edge to order by.
fn register_dummy_module_with_dep(registry: &Arc<Registry>, module_id: &str, depends_on: &str) {
    use apcore::registry::registry::{DependencyInfo, ModuleDescriptor};
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
        dependencies: vec![DependencyInfo {
            module_id: depends_on.to_string(),
            version_constraint: String::new(),
            optional: false,
        }],
        enabled: true,
    };
    registry
        .register_internal(module_id, module, descriptor)
        .expect("dummy registration");
}

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

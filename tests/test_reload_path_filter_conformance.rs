//! Drive `reload_path_filter.json` — granular reload via the `path_filter`
//! glob on `system.control.reload_module`
//! (docs/features/system-modules.md#14-granular-reload-via-path-filtering).
//!
//! `tests/test_reload_path_filter.rs` hand-transcribes two of these four cases;
//! this file replays the canonical fixture so a case added upstream reaches
//! Rust automatically.
//!
//! Response-shape note: the bulk (`path_filter`) branch answers with a
//! `reloaded_modules` array, while the single (`module_id`) branch answers with
//! a scalar `module_id` and no array — identical in apcore-python
//! (`src/apcore/sys_modules/control.py`). The fixture's `reloaded_modules_set`
//! is therefore read from the array when present and from `{module_id}`
//! otherwise; nothing is assumed when neither is present.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use apcore::context::{Context, Identity};
use apcore::events::emitter::EventEmitter;
use apcore::module::{Module, ModuleAnnotations};
use apcore::registry::registry::{ModuleDescriptor, Registry};
use apcore::sys_modules::control::ReloadModule;
use serde_json::Value;

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
        "Cannot find apcore conformance fixtures. Set APCORE_SPEC_REPO or clone \
         apcore as a sibling at {}",
        manifest_dir.parent().unwrap().join("apcore").display()
    );
}

fn fixture() -> Value {
    let path = find_fixtures_root().join("reload_path_filter.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    serde_json::from_str(&raw).expect("reload_path_filter.json parses")
}

fn dummy_ctx() -> Context<Value> {
    Context::<Value>::new(Identity::new(
        "@conformance".to_string(),
        "conformance".to_string(),
        vec![],
        HashMap::default(),
    ))
}

fn register_dummy(registry: &Arc<Registry>, id: &str) {
    struct Dummy;
    #[async_trait::async_trait]
    impl Module for Dummy {
        fn description(&self) -> &'static str {
            "conformance fixture module"
        }
        fn input_schema(&self) -> Value {
            serde_json::json!({})
        }
        fn output_schema(&self) -> Value {
            serde_json::json!({})
        }
        async fn execute(
            &self,
            _i: Value,
            _c: &Context<Value>,
        ) -> Result<Value, apcore::errors::ModuleError> {
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
        metadata: HashMap::new(),
        display: None,
        sunset_date: None,
        dependencies: vec![],
        enabled: true,
    };
    registry
        .register_internal(id, Box::new(Dummy), descriptor)
        .expect("register_internal");
}

/// Derive the fixture's `reloaded_modules_set` from a reload response.
fn reloaded_set(response: &Value) -> Vec<String> {
    if let Some(arr) = response.get("reloaded_modules").and_then(Value::as_array) {
        let mut ids: Vec<String> = arr
            .iter()
            .map(|v| {
                v.as_str()
                    .expect("reloaded_modules entry is a string")
                    .to_string()
            })
            .collect();
        ids.sort();
        return ids;
    }
    match response.get("module_id") {
        Some(Value::String(id)) => vec![id.clone()],
        _ => panic!(
            "reload response carries neither `reloaded_modules` nor a scalar \
             `module_id`, so the reloaded set cannot be determined: {response}"
        ),
    }
}

#[tokio::test]
async fn conformance_reload_path_filter() {
    let fx = fixture();
    let cases = fx["test_cases"].as_array().expect("test_cases is an array");
    assert!(!cases.is_empty(), "fixture must carry at least one case");

    for tc in cases {
        let id = tc["id"].as_str().expect("every case needs an id");

        let registry = Arc::new(Registry::new());
        for module_id in tc["registered_modules"]
            .as_array()
            .unwrap_or_else(|| panic!("[{id}] case has no registered_modules"))
        {
            register_dummy(
                &registry,
                module_id
                    .as_str()
                    .expect("registered module id is a string"),
            );
        }

        let reload = ReloadModule::new(Arc::clone(&registry), Arc::new(EventEmitter::new()));
        let outcome = reload.execute(tc["input"].clone(), &dummy_ctx()).await;

        let expected = tc["expected"]
            .as_object()
            .unwrap_or_else(|| panic!("[{id}] case has no expected object"));

        for (field, want) in expected {
            match field.as_str() {
                // Prose the fixture attaches to the expectation block.
                "_note" => {}
                "error_code" => {
                    let err = outcome
                        .as_ref()
                        .err()
                        .unwrap_or_else(|| panic!("[{id}] expected an error, got Ok"));
                    let actual = serde_json::to_value(err.code).expect("ErrorCode serializes");
                    assert_eq!(&actual, want, "[{id}] error_code");
                }
                "error" => {
                    // The fixture states `error: null` — the call must succeed.
                    assert!(want.is_null(), "[{id}] unexpected non-null `error` shape");
                    assert!(
                        outcome.is_ok(),
                        "[{id}] expected no error, got {:?}",
                        outcome.as_ref().err()
                    );
                }
                "success" => {
                    let response = outcome
                        .as_ref()
                        .unwrap_or_else(|e| panic!("[{id}] expected success, got {e:?}"));
                    assert_eq!(&response["success"], want, "[{id}] success");
                }
                "reloaded_modules_set" => {
                    let response = outcome
                        .as_ref()
                        .unwrap_or_else(|e| panic!("[{id}] expected success, got {e:?}"));
                    let mut want_ids: Vec<String> = want
                        .as_array()
                        .expect("reloaded_modules_set is an array")
                        .iter()
                        .map(|v| v.as_str().expect("module id is a string").to_string())
                        .collect();
                    want_ids.sort();
                    // An empty expected set means the response must report no
                    // reload at all, which the single-reload shape cannot express.
                    let actual = if want_ids.is_empty() {
                        response
                            .get("reloaded_modules")
                            .and_then(Value::as_array)
                            .map(|a| a.iter().map(std::string::ToString::to_string).collect())
                            .unwrap_or_else(|| {
                                panic!("[{id}] zero-match reload must still return an array")
                            })
                    } else {
                        reloaded_set(response)
                    };
                    assert_eq!(actual, want_ids, "[{id}] reloaded_modules_set");
                }
                other => panic!(
                    "[{id}] reload_path_filter.json grew expectation `{other}` that this \
                     driver does not check — teach the driver, do not skip it"
                ),
            }
        }
    }
}

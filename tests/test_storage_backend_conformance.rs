//! Drive `storage_backend.json` — the pluggable storage primitive shared by
//! ErrorHistory / UsageCollector / MetricsCollector (Issue #43, D-39).
//!
//! `tests/test_storage_backend.rs` asserts the same four-method contract by
//! hand; this file replays the canonical operation sequences instead, so a new
//! case in the fixture becomes a new assertion here for free.

use std::path::PathBuf;

use apcore::observability::storage::{InMemoryStorageBackend, StorageBackend};
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
    let path = find_fixtures_root().join("storage_backend.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    serde_json::from_str(&raw).expect("storage_backend.json parses")
}

/// Result of replaying one case's `operations` list.
#[derive(Default)]
struct Replay {
    raised_error: bool,
    /// Value returned by the most recent `get`, per namespace.
    last_get_by_ns: std::collections::HashMap<String, Option<Value>>,
    /// Value returned by the most recent `get`, regardless of namespace.
    last_get: Option<Value>,
    /// Keys returned by the most recent `list`.
    last_list_keys: Option<Vec<String>>,
}

async fn replay(ops: &[Value]) -> Replay {
    let backend = InMemoryStorageBackend::new();
    let mut out = Replay::default();

    for op in ops {
        let name = op["op"].as_str().expect("every operation needs an `op`");
        let ns = op["namespace"].as_str().unwrap_or_default();
        match name {
            "save" => {
                let key = op["key"].as_str().expect("save needs a key");
                if backend.save(ns, key, op["value"].clone()).await.is_err() {
                    out.raised_error = true;
                }
            }
            "get" => {
                let key = op["key"].as_str().expect("get needs a key");
                match backend.get(ns, key).await {
                    Ok(v) => {
                        out.last_get_by_ns.insert(ns.to_string(), v.clone());
                        out.last_get = Some(v.unwrap_or(Value::Null));
                    }
                    Err(_) => out.raised_error = true,
                }
            }
            "list" => {
                let prefix = op["prefix"].as_str().unwrap_or("");
                match backend.list(ns, prefix).await {
                    Ok(entries) => {
                        let mut keys: Vec<String> = entries.into_iter().map(|(k, _)| k).collect();
                        keys.sort();
                        out.last_list_keys = Some(keys);
                    }
                    Err(_) => out.raised_error = true,
                }
            }
            "delete" => {
                let key = op["key"].as_str().expect("delete needs a key");
                if backend.delete(ns, key).await.is_err() {
                    out.raised_error = true;
                }
            }
            other => panic!(
                "storage_backend.json grew operation `{other}` that this driver \
                 cannot execute — teach the driver, do not skip the case"
            ),
        }
    }
    out
}

#[tokio::test]
async fn conformance_storage_backend() {
    let fx = fixture();
    let cases = fx["test_cases"].as_array().expect("test_cases is an array");
    assert!(!cases.is_empty(), "fixture must carry at least one case");

    for tc in cases {
        let id = tc["id"].as_str().expect("every case needs an id");
        let ops = tc["input"]["operations"]
            .as_array()
            .unwrap_or_else(|| panic!("[{id}] case has no input.operations"));
        let got = replay(ops).await;

        let expected = tc["expected"]
            .as_object()
            .unwrap_or_else(|| panic!("[{id}] case has no expected object"));

        // Every expectation the fixture states must be checked here. An
        // unrecognised key panics rather than being ignored, so a new
        // assertion in the canonical fixture cannot land as a silent pass.
        for (field, want) in expected {
            match field.as_str() {
                "final_get_value" => {
                    let actual = got.last_get.clone().unwrap_or_else(|| {
                        panic!("[{id}] expects final_get_value but no get() ran")
                    });
                    assert_eq!(&actual, want, "[{id}] final_get_value");
                }
                "matched_keys_sorted" => {
                    let actual = got
                        .last_list_keys
                        .clone()
                        .unwrap_or_else(|| panic!("[{id}] expects list() results but none ran"));
                    let want_keys: Vec<String> = want
                        .as_array()
                        .expect("matched_keys_sorted is an array")
                        .iter()
                        .map(|v| v.as_str().expect("key is a string").to_string())
                        .collect();
                    assert_eq!(actual, want_keys, "[{id}] matched_keys_sorted");
                }
                "raised_error" => {
                    assert_eq!(
                        got.raised_error,
                        want.as_bool().expect("raised_error is a bool"),
                        "[{id}] raised_error"
                    );
                }
                "errors_namespace_value" | "metrics_namespace_value" => {
                    let ns = field.trim_end_matches("_namespace_value");
                    let actual = got
                        .last_get_by_ns
                        .get(ns)
                        .unwrap_or_else(|| panic!("[{id}] no get() ran against namespace `{ns}`"))
                        .clone()
                        .unwrap_or(Value::Null);
                    assert_eq!(&actual, want, "[{id}] {field}");
                }
                other => panic!(
                    "[{id}] storage_backend.json grew expectation `{other}` that this \
                     driver does not check — teach the driver, do not skip it"
                ),
            }
        }
    }
}

//! Drive `bindings_dir_resolution.json` — the binding-directory resolution
//! contract (PROTOCOL_SPEC §5.12.6, aiperceivable/apcore#114).
//!
//! The shape that matters is a `bindings.dir` declared in a config FILE with no
//! explicit directory argument at the loader. Every pre-existing
//! `load_binding_dir` test in this SDK passes a directory explicitly, and that
//! is the one path which behaved identically before and after #114, so none of
//! them can tell the corrected behaviour from the status quo.
//!
//! Entry point: `BindingLoader::load_binding_dir_with_config`, the public
//! loader whose directory argument is an `Option`. It is the Rust spelling of
//! apcore-python's `load_binding_dir()` — `load_binding_dir(dir, pattern)`
//! delegates to it with `Some(dir)` and cannot express "no explicit directory",
//! which is the fixture's `explicit_dir: null`. No private resolution helper is
//! touched, per the fixture's `entry_point` clause.
//!
//! Kept out of `tests/it.rs` and declared as its own `[[test]]` binary: every
//! case mutates `APCORE_BINDINGS_*` and the process working directory, which
//! `it.rs`'s threads would share.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use apcore::bindings::BindingLoader;
use apcore::config::Config;
use apcore::APCore;
use serde_json::Value;

#[path = "conformance_env.rs"]
mod conformance_env;

use crate::conformance_env::find_fixtures_root;

const FIXTURE: &str = "bindings_dir_resolution.json";

/// The `APCORE_*` variables this fixture's precedence chain runs through.
///
/// Removed — never set to `""` — before every case that does not name them:
/// §9.2 makes an empty `APCORE_BINDINGS_DIR` a declaration of `bindings.dir`
/// with an empty value, which is an override, not a neutral state.
const OWNED_ENV: &[&str] = &[
    "APCORE_BINDINGS_DIR",
    "APCORE_BINDINGS_PATTERN",
    "APCORE_CONFIG_FILE",
];

/// Cases this SDK does not satisfy as the fixture states them. Driven by the
/// `#[ignore]`d test below rather than dropped, so the divergence is visible in
/// `cargo test` output and fails loudly under `--ignored`.
///
/// `missing_configured_dir_is_not_an_error`: the fixture expects a configured
/// `bindings.dir` that does not exist to yield an empty result with no error.
/// This SDK raises `BINDING_FILE_INVALID` for a non-directory, on the explicit
/// and the config-resolved path alike (`BindingLoader::load_binding_dir_with_
/// config`), and §5.12.6 states no requirement either way. Reported upstream
/// rather than repaired here: changing it is a behaviour change to a public
/// entry point, not a test.
const DIVERGENT: &[&str] = &["missing_configured_dir_is_not_an_error"];

/// Serialises the cases against each other: each one sets `APCORE_BINDINGS_*`
/// and `chdir`s into its own layout, both of which are process-global.
static ENV_GUARD: Mutex<()> = Mutex::new(());

fn env_guard() -> MutexGuard<'static, ()> {
    ENV_GUARD.lock().unwrap_or_else(PoisonError::into_inner)
}

fn fixture() -> Value {
    let path = find_fixtures_root().join(FIXTURE);
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{FIXTURE} parses: {e}"))
}

fn clear_owned_env() {
    for name in OWNED_ENV {
        std::env::remove_var(name);
    }
}

/// The module ID a fixture `fs` entry stands for: the file name up to its first
/// dot.
///
/// The fixture writes one shared descriptor into differently-NAMED files
/// (`greet.binding.yaml`, `file_side.binding.yaml`, `decoy.binding.yaml`) and
/// then expects different `loaded_module_ids` per case, so the name is what
/// distinguishes the candidates. A driver that wrote `module_id: greet` into
/// every file would make each case's decoys indistinguishable from its winner.
fn module_id_for(relative: &str) -> String {
    Path::new(relative)
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.split('.').next())
        .unwrap_or_else(|| panic!("{relative} has a file name"))
        .to_string()
}

/// Write the fixture's binding descriptor to `root/relative`.
///
/// One field is translated: the fixture spells the target `target_id`
/// (PROTOCOL_SPEC §5.12.2), while this SDK's `BindingEntry` spells it `target`.
/// That naming divergence is pre-existing and outside §5.12.6's subject — this
/// fixture asserts which directory is scanned, not the descriptor's field
/// names — so the driver maps it and reports it rather than failing every case
/// on it.
fn write_binding_file(root: &Path, relative: &str, template: &Value) {
    let target = root.join(relative);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).expect("create binding dir");
    }

    let mut document = template.clone();
    let entries = document["bindings"]
        .as_array_mut()
        .expect("binding_file declares a bindings array");
    for entry in entries.iter_mut() {
        let object = entry.as_object_mut().expect("binding entry is an object");
        if let Some(target_id) = object.remove("target_id") {
            object.insert("target".to_string(), target_id);
        }
        object.insert(
            "module_id".to_string(),
            Value::String(module_id_for(relative)),
        );
    }

    let yaml = serde_yaml_ng::to_string(&document).expect("binding descriptor serializes");
    std::fs::write(&target, yaml).expect("write binding file");
}

/// Write the case's `config_file` block as a real YAML document on disk.
///
/// The fixture's content is used verbatim except for the two §9.1 required
/// fields (`version`, `project.name`), which every case omits and which this
/// SDK enforces inside `Config::load`. They carry no path-typed value and
/// change nothing this fixture asserts; without them the document is invalid
/// and no case could reach the loader at all.
fn write_config_file(root: &Path, block: &Value) -> PathBuf {
    let relative = block["path"].as_str().expect("config_file names a path");
    let mut content = block["content"].clone();
    let object = content
        .as_object_mut()
        .expect("config_file content is an object");
    object
        .entry("version")
        .or_insert_with(|| Value::String("1.0.0".to_string()));
    object
        .entry("project")
        .or_insert_with(|| serde_json::json!({ "name": "bindings-dir-resolution-conformance" }));

    let path = root.join(relative);
    let yaml = serde_yaml_ng::to_string(&content).expect("config serializes");
    std::fs::write(&path, yaml).expect("write config file");
    path
}

/// Materialise a case's `fs` block and return `module_id -> directory`, the map
/// that turns an observed load back into the directory it was scanned from.
fn materialise_fs(root: &Path, fs: &Value, template: &Value, id: &str) -> BTreeMap<String, String> {
    let mut origins = BTreeMap::new();
    let empty = serde_json::Map::new();
    for (relative, kind) in fs.as_object().unwrap_or(&empty) {
        match kind.as_str().expect("fs value is a string") {
            "binding_file" => {
                write_binding_file(root, relative, template);
                let dir = Path::new(relative)
                    .parent()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default();
                origins.insert(module_id_for(relative), dir);
            }
            other => panic!(
                "FAIL [{id}]: {FIXTURE} grew fs kind `{other}` that this driver cannot \
                 materialise — teach the driver, do not skip it"
            ),
        }
    }
    origins
}

/// Run one case's loader invocation and report `(module_ids, scanned_dirs)`.
///
/// `scanned_dir` is read back from the filesystem layout — which directory held
/// the files that actually loaded — never from the config value the driver
/// supplied, per the fixture's `scan_observation` clause.
fn observe_load(
    config: &Config,
    explicit_dir: Option<&str>,
    origins: &BTreeMap<String, String>,
) -> Result<(Vec<String>, Vec<String>), apcore::errors::ModuleError> {
    let mut loader = BindingLoader::new();
    let explicit = explicit_dir.map(PathBuf::from);
    loader.load_binding_dir_with_config(explicit.as_deref(), None, Some(config))?;

    let mut module_ids: Vec<String> = loader
        .list_bindings()
        .into_iter()
        .map(std::string::ToString::to_string)
        .collect();
    module_ids.sort();

    let mut scanned: Vec<String> = module_ids
        .iter()
        .map(|id| {
            origins
                .get(id)
                .unwrap_or_else(|| panic!("loaded `{id}` came from no directory this driver wrote"))
                .clone()
        })
        .collect();
    scanned.sort();
    scanned.dedup();
    Ok((module_ids, scanned))
}

fn expected_ids(expected: &serde_json::Map<String, Value>, id: &str) -> Vec<String> {
    expected["loaded_module_ids"]
        .as_array()
        .unwrap_or_else(|| panic!("[{id}] loaded_module_ids is an array"))
        .iter()
        .map(|v| v.as_str().expect("module id is a string").to_string())
        .collect()
}

/// Set up one case's layout, environment and working directory.
///
/// Returns the temp dir (kept alive by the caller), the config it loaded and
/// the `module_id -> directory` map.
fn prepare(
    tc: &Value,
    template: &Value,
    id: &str,
) -> (tempfile::TempDir, Config, BTreeMap<String, String>) {
    let workspace = tempfile::tempdir().expect("tempdir");
    let root = workspace.path().to_path_buf();

    clear_owned_env();
    let empty = serde_json::Map::new();
    for (name, value) in tc["env"].as_object().unwrap_or(&empty) {
        assert!(
            OWNED_ENV.contains(&name.as_str()),
            "[{id}] case sets {name}, which this driver does not isolate"
        );
        std::env::set_var(name, value.as_str().expect("env value is a string"));
    }

    let origins = materialise_fs(&root, &tc["fs"], template, id);
    let config_path = write_config_file(&root, &tc["config_file"]);

    // The case's paths (`./custom_bindings`, `from_argument`) are relative to
    // the layout root, which is therefore where the process has to stand.
    std::env::set_current_dir(&root).expect("chdir into the case layout");

    let config = Config::load(&config_path)
        .unwrap_or_else(|e| panic!("[{id}] the case's config file must load: {e:?}"));
    (workspace, config, origins)
}

#[test]
fn conformance_bindings_dir_resolution() {
    let _guard = env_guard();
    let original_cwd = std::env::current_dir().expect("cwd");
    let fx = fixture();
    let template = fx["binding_file"].clone();
    let cases = fx["test_cases"].as_array().expect("test_cases is an array");
    assert_eq!(cases.len(), 8, "driver is written against all 8 cases");

    let ids: Vec<&str> = cases
        .iter()
        .map(|tc| tc["id"].as_str().expect("every case needs an id"))
        .collect();
    for divergent in DIVERGENT {
        assert!(
            ids.contains(divergent),
            "the divergence note names `{divergent}`, which {FIXTURE} no longer declares — \
             re-check whether it still applies"
        );
    }

    for tc in cases {
        let id = tc["id"].as_str().expect("every case needs an id");
        if DIVERGENT.contains(&id) {
            continue;
        }
        let expected = tc["expected"]
            .as_object()
            .unwrap_or_else(|| panic!("[{id}] case has no expected object"));

        let (workspace, config, origins) = prepare(tc, &template, id);

        if tc["invoke_loader"].as_bool() == Some(false) {
            // §5.12.6 clause 3: constructing a client MUST NOT scan. The
            // configured directory holds a well-formed binding file that would
            // load cleanly if anything scanned it, so the observable claim is
            // that its module ID is absent from the registry.
            assert_eq!(
                expected["scanned"].as_bool(),
                Some(false),
                "[{id}] a no-loader case must expect no scan"
            );
            let client = APCore::with_options(None, None, Some(config), None);
            let registered = client.list_modules(None, None);
            assert_eq!(
                registered,
                Vec::<String>::new(),
                "[{id}] registered_module_ids (expected {})",
                expected["registered_module_ids"]
            );
            for module_id in origins.keys() {
                assert!(
                    !registered.contains(module_id),
                    "[{id}] client construction scanned the binding directory"
                );
            }
            std::env::set_current_dir(&original_cwd).expect("restore cwd");
            drop(workspace);
            continue;
        }

        assert_eq!(
            expected["scanned"].as_bool(),
            Some(true),
            "[{id}] this driver expects a scan for every loader case"
        );
        // `explicit_dir: null` MUST reach the loader as a genuinely absent
        // argument: a directory the driver computed itself works under BOTH the
        // pre-#114 and the corrected behaviour.
        let explicit_dir = tc["explicit_dir"].as_str();
        let (module_ids, scanned) = observe_load(&config, explicit_dir, &origins)
            .unwrap_or_else(|e| panic!("[{id}] the loader must succeed: {e:?}"));

        assert_eq!(
            module_ids,
            expected_ids(expected, id),
            "[{id}] loaded_module_ids"
        );

        let want_dir = expected["scanned_dir"]
            .as_str()
            .unwrap_or_else(|| panic!("[{id}] case states no scanned_dir"));
        assert_eq!(
            scanned,
            vec![want_dir.to_string()],
            "[{id}] scanned_dir: the loader enumerated the wrong directory"
        );

        std::env::set_current_dir(&original_cwd).expect("restore cwd");
        drop(workspace);
    }

    clear_owned_env();
    std::env::set_current_dir(&original_cwd).expect("restore cwd");
}

/// The `missing_configured_dir_is_not_an_error` case, driven exactly as the
/// fixture states it.
///
/// IGNORED, not deleted: this SDK returns `BINDING_FILE_INVALID` for a
/// configured `bindings.dir` that does not exist, where the fixture expects an
/// empty result and no error. §5.12.6 requires neither, so this is a genuine
/// fixture-vs-SDK divergence to settle upstream. Run it with
/// `cargo test --test test_bindings_dir_resolution_conformance -- --ignored`
/// to see the current behaviour.
#[test]
#[ignore = "divergence: this SDK errors on a missing bindings.dir; the fixture expects an empty result (apcore#114)"]
fn conformance_bindings_dir_resolution_missing_dir() {
    let _guard = env_guard();
    let original_cwd = std::env::current_dir().expect("cwd");
    let fx = fixture();
    let template = fx["binding_file"].clone();
    let case = fx["test_cases"]
        .as_array()
        .expect("test_cases is an array")
        .iter()
        .find(|tc| tc["id"] == Value::String(DIVERGENT[0].to_string()))
        .unwrap_or_else(|| panic!("{FIXTURE} declares {}", DIVERGENT[0]));
    let id = DIVERGENT[0];

    let (workspace, config, origins) = prepare(case, &template, id);
    let outcome = observe_load(&config, case["explicit_dir"].as_str(), &origins);

    std::env::set_current_dir(&original_cwd).expect("restore cwd");
    clear_owned_env();
    drop(workspace);

    let expected = case["expected"].as_object().expect("expected object");
    assert!(
        expected["error"].is_null(),
        "[{id}] the fixture expects no error"
    );
    let (module_ids, _) =
        outcome.unwrap_or_else(|e| panic!("[{id}] a missing bindings.dir must not error: {e:?}"));
    assert_eq!(
        module_ids,
        expected_ids(expected, id),
        "[{id}] loaded_module_ids"
    );
}

/// Pin the divergence to exactly a raise, so it cannot quietly widen.
///
/// The companion to the `#[ignore]`d case above: `bindings.dir` still resolves
/// through the config tier when the directory is missing — the error names the
/// CONFIGURED directory, not the `./bindings` default — and the difference from
/// the fixture is confined to "raises instead of returning empty". apcore-python
/// and apcore-typescript record the same divergence, so this is a fixture-side
/// question, not a Rust one.
#[test]
fn missing_configured_dir_still_resolves_through_the_config_tier() {
    let _guard = env_guard();
    let original_cwd = std::env::current_dir().expect("cwd");
    let fx = fixture();
    let template = fx["binding_file"].clone();
    let id = DIVERGENT[0];
    let case = fx["test_cases"]
        .as_array()
        .expect("test_cases is an array")
        .iter()
        .find(|tc| tc["id"] == Value::String(id.to_string()))
        .unwrap_or_else(|| panic!("{FIXTURE} declares {id}"))
        .clone();

    let configured = case["config_file"]["content"]["bindings"]["dir"]
        .as_str()
        .expect("the case configures a bindings.dir")
        .to_string();

    let (workspace, config, origins) = prepare(&case, &template, id);
    let outcome = observe_load(&config, case["explicit_dir"].as_str(), &origins);

    std::env::set_current_dir(&original_cwd).expect("restore cwd");
    clear_owned_env();
    drop(workspace);

    let error = outcome.expect_err("this SDK raises for a missing binding directory");
    assert_eq!(
        error.code,
        apcore::errors::ErrorCode::BindingFileInvalid,
        "the divergence is a BINDING_FILE_INVALID raise and nothing else"
    );
    assert!(
        error.message.contains(configured.trim_start_matches("./")),
        "the configured directory must be the one attempted, not the default: {}",
        error.message
    );
}

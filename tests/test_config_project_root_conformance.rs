//! Drive `config_project_root.json` — the project root and its narrow
//! deprecation warning (PROTOCOL_SPEC §9.2.2, aiperceivable/apcore#113).
//!
//! v1.35.0 is the DEPRECATION phase: the accessor is required, the warning is a
//! SHOULD, and no resolution behaviour changes. Nothing here asserts that a
//! relative path-typed value resolves against the project root — the
//! `v1x_current_bases_unchanged` case pins the opposite, and this SDK is
//! expected to keep `acl.root` anchored at the config file's directory and
//! `schema.root` at CWD for the whole 1.x line.
//!
//! Every tier case reaches the SDK through DISCOVERY (`Config::discover()`,
//! the no-path load), because the §9.14 tier is the input under test.
//!
//! Kept out of `tests/it.rs` and declared as its own `[[test]]` binary: each
//! case sets `$HOME`, `APCORE_*` and the process working directory, all of
//! which `it.rs`'s threads would share.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use apcore::acl::ACL;
use apcore::config::Config;
use apcore::schema::loader::SchemaLoader;
use serde_json::Value;

#[path = "conformance_env.rs"]
mod conformance_env;

use crate::conformance_env::find_fixtures_root;

const FIXTURE: &str = "config_project_root.json";

/// The `APCORE_*` variables these cases run through.
///
/// Removed — never set to `""` — before every case that does not name them.
/// §9.2 makes an empty `APCORE_CONFIG_FILE` or `APCORE_ACL_ROOT` a declaration
/// with an empty value, so blanking is an override, not isolation.
const OWNED_ENV: &[&str] = &[
    "APCORE_CONFIG_FILE",
    "APCORE_ACL_ROOT",
    "APCORE_SCHEMA_ROOT",
    "APCORE_EXTENSIONS_ROOT",
    "APCORE_BINDINGS_DIR",
];

/// Cases this SDK does not satisfy as the fixture states them. Driven by the
/// `#[ignore]`d test below rather than dropped, so the divergence shows up in
/// `cargo test` output and fails loudly under `--ignored`.
///
/// `no_warning_when_all_path_values_absolute`: the case's config declares
/// `schema.root` and `acl.root` absolute but leaves `extensions.root` and
/// `bindings.dir` undeclared, so their §9.1.1 relative defaults (`./extensions`,
/// `./bindings`) still stand in the merged configuration. §9.2.2's target
/// semantics clause 2 says a relative DEFAULT re-roots exactly as a written
/// value does, so this SDK counts them, finds a relative path-typed value
/// present, and warns. The case's precondition
/// (`relative_path_typed_values_present: false`) is not established by the
/// config it writes. Reported upstream rather than papered over: the SHOULD in
/// requirement 2 is satisfied by the SDK's reading, and narrowing the warning
/// to file-declared values only would contradict clause 2.
const DIVERGENT: &[&str] = &["no_warning_when_all_path_values_absolute"];

/// Serialises the cases: each sets `$HOME`, `APCORE_*` and the working
/// directory, all process-global.
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

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

/// The fixture spells §9.14 tier 6 as `~/.config/apcore/`, which §9.14 itself
/// glosses as "XDG on Linux, `~/Library/Application Support` on macOS". This
/// maps the fixture's path onto the location this platform's discovery actually
/// searches; without it the tier-6 case would test that a file nobody looks for
/// is not found.
fn translate(relative: &str) -> String {
    const FIXTURE_TIER6: &str = "fakehome/.config/apcore";
    if cfg!(target_os = "macos") {
        if let Some(rest) = relative.strip_prefix(FIXTURE_TIER6) {
            return format!("fakehome/Library/Application Support/apcore{rest}");
        }
    }
    relative.to_string()
}

fn canon(path: &Path) -> PathBuf {
    // macOS temp directories are reached through a /private symlink, so both
    // sides of every path comparison are normalised.
    path.canonicalize()
        .unwrap_or_else(|e| panic!("canonicalize {}: {e}", path.display()))
}

/// Write one `fs` entry verbatim, except for the two §9.1 required fields.
///
/// Every configuration document in this fixture is spelled `project: {name:
/// fixture}` with no `version`, which `Config::load` rejects. The line is
/// prepended rather than the document rewritten, so the fixture's own YAML —
/// including every path-typed value the case is about — reaches disk unchanged.
fn write_fs_entry(root: &Path, relative: &str, content: &str) {
    let target = root.join(translate(relative));
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).expect("create fs parent");
    }
    let body = if content.contains("project:") && !content.contains("version:") {
        format!("version: '1.0.0'\n{content}\n")
    } else {
        content.to_string()
    };
    std::fs::write(&target, body).expect("write fs entry");
}

fn build_layout(root: &Path, fx: &Value) {
    for dir in fx["layout"]["dirs"]
        .as_array()
        .expect("layout declares dirs")
    {
        let dir = dir.as_str().expect("dir is a string");
        std::fs::create_dir_all(root.join(translate(dir))).expect("create layout dir");
    }
}

// ---------------------------------------------------------------------------
// Warning capture
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
struct CaptureWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for CaptureWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl tracing_subscriber::fmt::MakeWriter<'_> for CaptureWriter {
    type Writer = Self;
    fn make_writer(&self) -> Self::Writer {
        self.clone()
    }
}

/// Observe the SDK's real warning channel — `tracing` — around one load, per
/// the fixture's `warning_observation` clause. Presence only: the text is not
/// normative.
fn capture_logs(f: impl FnOnce()) -> String {
    let buffer = CaptureWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(buffer.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .finish();
    tracing::subscriber::with_default(subscriber, f);
    let bytes = buffer
        .0
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    String::from_utf8_lossy(&bytes).into_owned()
}

// ---------------------------------------------------------------------------
// Observations
// ---------------------------------------------------------------------------

/// The §9.2.2 requirement-2 predicate, computed through public API only:
/// is any path-typed value in the MERGED configuration relative?
///
/// This SDK's own helper (`Config::relative_path_typed_keys`) is private, so the
/// driver restates the predicate over the public `Config::path_typed_keys()` and
/// `Config::get`. Reading through `get` is what makes a defaulted `./schemas`
/// count, which §9.2.2's target-semantics clause 2 requires.
fn relative_path_typed_keys(config: &Config) -> Vec<String> {
    fn is_relative(value: Option<&str>) -> bool {
        value.is_some_and(|s| !s.is_empty() && Path::new(s).is_relative())
    }

    let mut hits = Vec::new();
    for key in Config::path_typed_keys() {
        let hit = match key.strip_suffix("[]") {
            Some(list_key) => matches!(
                config.get(list_key),
                Some(Value::Array(ref items)) if items.iter().any(|item| is_relative(match item {
                    Value::String(s) => Some(s.as_str()),
                    Value::Object(o) => o.get("root").and_then(Value::as_str),
                    _ => None,
                }))
            ),
            None => is_relative(config.get(key).as_ref().and_then(Value::as_str)),
        };
        if hit {
            hits.push((*key).to_string());
        }
    }
    hits
}

/// Which of the two candidate directories `ACL::discover` actually anchored at.
///
/// Read by removing the policy file under the directory the fixture names and
/// re-running discovery: the other candidate's policy file is still in place, so
/// an SDK anchored at CWD would keep finding one. The two policy documents the
/// fixture writes are byte-identical, which is why presence, not content, is the
/// observable.
fn assert_acl_root(config: &Config, root: &Path, want: &str, id: &str) {
    let policy = root.join(want).join("global_acl.yaml");
    assert!(
        policy.is_file(),
        "[{id}] precondition: the fixture layout must put a policy at {}",
        policy.display()
    );

    let before = ACL::discover(config).expect("acl discovery must not error");
    assert!(
        before.is_some(),
        "[{id}] resolved_acl_root: no ACL was discovered at all"
    );

    std::fs::remove_file(&policy).expect("remove the policy under the expected root");
    let after = ACL::discover(config).expect("acl discovery must not error");
    std::fs::write(&policy, "default_effect: deny\nrules: []\n").expect("restore policy");

    assert!(
        after.is_none(),
        "[{id}] resolved_acl_root: discovery survived removing {}, so it anchored somewhere else",
        policy.display()
    );
}

/// Which of the two candidate directories `SchemaLoader` resolves `schema.root`
/// against.
///
/// The fixture creates both `schemas/` directories (via `.keep`) precisely so
/// exactly one semantics passes; the loader needs a file to find, so the driver
/// drops a marker schema into each and reads back which one loaded.
fn assert_schema_root(config: &Config, root: &Path, want: &str, candidates: &[&str], id: &str) {
    for candidate in candidates {
        let dir = root.join(candidate);
        assert!(
            dir.is_dir(),
            "[{id}] precondition: the fixture layout must create {}",
            dir.display()
        );
        std::fs::write(
            dir.join("probe.schema.yaml"),
            format!("description: {candidate}\ntype: object\n"),
        )
        .expect("write probe schema");
    }

    let mut loader = SchemaLoader::with_config(config, None);
    let definition = loader
        .load("probe")
        .unwrap_or_else(|e| panic!("[{id}] the probe schema must load: {e:?}"));
    assert_eq!(
        definition.description, want,
        "[{id}] resolved_schema_root: schema.root resolved against the wrong base"
    );
}

// ---------------------------------------------------------------------------
// Case runner
// ---------------------------------------------------------------------------

struct CaseEnv {
    home: Option<String>,
    xdg: Option<String>,
    cwd: PathBuf,
}

impl CaseEnv {
    fn capture() -> Self {
        Self {
            home: std::env::var("HOME").ok(),
            xdg: std::env::var("XDG_CONFIG_HOME").ok(),
            cwd: std::env::current_dir().expect("cwd"),
        }
    }

    fn restore(&self) {
        match &self.home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
        match &self.xdg {
            Some(value) => std::env::set_var("XDG_CONFIG_HOME", value),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        for name in OWNED_ENV {
            std::env::remove_var(name);
        }
        std::env::set_current_dir(&self.cwd).expect("restore cwd");
    }
}

/// Materialise one case: layout, redirected home, environment, working
/// directory. Returns the loaded `Config` and whatever the load logged.
fn run_case(fx: &Value, tc: &Value, root: &Path, id: &str) -> (Config, String) {
    build_layout(root, fx);

    // Home isolation: tiers 6-7 must never read the real user's home, and no
    // other case may pick up a config file that lives there.
    std::env::set_var("HOME", root.join("fakehome"));
    std::env::remove_var("XDG_CONFIG_HOME");
    for name in OWNED_ENV {
        std::env::remove_var(name);
    }

    let empty = serde_json::Map::new();
    for (name, value) in tc["env"].as_object().unwrap_or(&empty) {
        assert!(
            OWNED_ENV.contains(&name.as_str()),
            "[{id}] case sets {name}, which this driver does not isolate"
        );
        let value = value.as_str().expect("env value is a string");
        if name == "APCORE_CONFIG_FILE" {
            // The fixture's paths are relative to the layout root; CWD is
            // `project/`, so the selector has to be absolutised or tier 1 would
            // resolve against the wrong directory.
            std::env::set_var(name, root.join(translate(value)));
        } else {
            std::env::set_var(name, value);
        }
    }

    for (relative, content) in tc["fs"].as_object().unwrap_or(&empty) {
        write_fs_entry(
            root,
            relative,
            content.as_str().expect("fs content is text"),
        );
    }

    // `cwd_must_differ`: the process CWD is `project/`, never the directory the
    // configuration file lives in for the cases that discriminate.
    let cwd = tc["cwd"]
        .as_str()
        .or_else(|| fx["layout"]["cwd"].as_str())
        .expect("a case or the layout names a cwd");
    std::env::set_current_dir(root.join(cwd)).expect("chdir into the layout cwd");

    if let Some(mapping) = tc.get("config_from_mapping") {
        // No discovery ran and there is no source path.
        let config: Config = serde_json::from_value(mapping.clone())
            .unwrap_or_else(|e| panic!("[{id}] the case's mapping deserializes: {e}"));
        return (config, String::new());
    }

    let mut loaded = None;
    let logs = capture_logs(|| {
        loaded = Some(
            Config::discover().unwrap_or_else(|e| panic!("[{id}] discovery must not error: {e:?}")),
        );
    });
    (loaded.expect("discovery produced a config"), logs)
}

#[allow(clippy::too_many_lines)] // one arm per expectation key, each asserted where the fixture states it
fn assert_case(fx: &Value, tc: &Value, root: &Path, id: &str) {
    let (config, logs) = run_case(fx, tc, root, id);
    let cwd = std::env::current_dir().expect("cwd");
    let expected = tc["expected"]
        .as_object()
        .unwrap_or_else(|| panic!("[{id}] case has no expected object"));

    for (field, want) in expected {
        match field.as_str() {
            "config_source_dir" => {
                let actual = config.source_path().map(|source| {
                    canon(source)
                        .parent()
                        .expect("a file has a parent")
                        .to_path_buf()
                });
                match want.as_str() {
                    Some(relative) => {
                        let expect = canon(&root.join(translate(relative)));
                        assert_eq!(
                            actual,
                            Some(expect),
                            "[{id}] config_source_dir (source_path={:?})",
                            config.source_path()
                        );
                    }
                    None => assert!(
                        want.is_null(),
                        "[{id}] config_source_dir must be a string or null"
                    ),
                }
                if want.is_null() {
                    assert_eq!(actual, None, "[{id}] config_source_dir must be absent");
                }
            }

            "project_root" => {
                let relative = want.as_str().expect("project_root is a string");
                assert_eq!(
                    canon(&config.project_root()),
                    canon(&root.join(translate(relative))),
                    "[{id}] project_root"
                );
            }

            "project_root_equals_cwd" => {
                let same = canon(&config.project_root()) == canon(&cwd);
                assert_eq!(
                    same,
                    want.as_bool().expect("project_root_equals_cwd is a bool"),
                    "[{id}] project_root_equals_cwd (project_root={}, cwd={})",
                    config.project_root().display(),
                    cwd.display()
                );
            }

            "relative_path_typed_values_present" => {
                let hits = relative_path_typed_keys(&config);
                assert_eq!(
                    !hits.is_empty(),
                    want.as_bool()
                        .expect("relative_path_typed_values_present is a bool"),
                    "[{id}] relative_path_typed_values_present (relative keys: {hits:?})"
                );
            }

            "deprecation_warning" => {
                let warned = logs.contains("DEPRECATION");
                assert_eq!(
                    warned,
                    want.as_bool().expect("deprecation_warning is a bool"),
                    "[{id}] deprecation_warning; captured logs:\n{logs}"
                );
                if warned {
                    // The warning must be about this migration, not some other
                    // deprecation that happened to fire during the load.
                    assert!(
                        logs.contains("aiperceivable/apcore#113"),
                        "[{id}] the warning must name the issue that explains it:\n{logs}"
                    );
                }
            }

            "resolved_acl_root" => {
                assert_acl_root(
                    &config,
                    root,
                    want.as_str().expect("resolved_acl_root is a string"),
                    id,
                );
            }

            "resolved_schema_root" => {
                // Both candidates come from the case's own `fs` block: the
                // fixture creates `schemas/` under BOTH `project/` and
                // `elsewhere/` so exactly one semantics passes.
                assert_schema_root(
                    &config,
                    root,
                    want.as_str().expect("resolved_schema_root is a string"),
                    &["project/schemas", "elsewhere/schemas"],
                    id,
                );
            }

            other => panic!(
                "FAIL [{id}]: {FIXTURE} grew expectation `{other}` that this driver does \
                 not assert — teach the driver, do not skip it"
            ),
        }
    }
}

#[test]
fn conformance_config_project_root() {
    let _guard = env_guard();
    let saved = CaseEnv::capture();
    let fx = fixture();
    let cases = fx["test_cases"].as_array().expect("test_cases is an array");
    assert_eq!(cases.len(), 14, "driver is written against all 14 cases");

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
        let workspace = tempfile::tempdir().expect("tempdir");
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            assert_case(&fx, tc, workspace.path(), id);
        }));
        saved.restore();
        drop(workspace);
        if let Err(payload) = outcome {
            std::panic::resume_unwind(payload);
        }
    }

    saved.restore();
}

/// The `no_warning_when_all_path_values_absolute` case, driven exactly as the
/// fixture states it.
///
/// IGNORED, not deleted: the case's config leaves `extensions.root` and
/// `bindings.dir` undeclared, so their relative §9.1.1 defaults stand and this
/// SDK — which counts defaults, as §9.2.2's clause 2 requires — reports a
/// relative path-typed value present and warns. Run it with
/// `cargo test --test test_config_project_root_conformance -- --ignored`
/// to see the divergence.
#[test]
#[ignore = "divergence: the case's config leaves relative path-typed DEFAULTS standing, which this SDK counts (apcore#113)"]
fn conformance_config_project_root_all_absolute() {
    let _guard = env_guard();
    let saved = CaseEnv::capture();
    let fx = fixture();
    let id = DIVERGENT[0];
    let case = fx["test_cases"]
        .as_array()
        .expect("test_cases is an array")
        .iter()
        .find(|tc| tc["id"] == Value::String(id.to_string()))
        .unwrap_or_else(|| panic!("{FIXTURE} declares {id}"))
        .clone();

    let workspace = tempfile::tempdir().expect("tempdir");
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assert_case(&fx, &case, workspace.path(), id);
    }));
    saved.restore();
    drop(workspace);
    if let Err(payload) = outcome {
        std::panic::resume_unwind(payload);
    }
}

/// The substantive half of §9.2.2 requirement 2's second negative, driven green.
///
/// The companion to the `#[ignore]`d case above: with EVERY §9.2.1 key spelled
/// absolutely — including the three that otherwise stand at their relative
/// §9.1.1 defaults — a project root outside CWD must produce no warning. Without
/// this, the divergence note would leave the requirement's second negative
/// unasserted, and an implementation that warns on the project-root difference
/// alone would pass everything that remains.
#[test]
fn no_warning_when_every_path_typed_value_is_genuinely_absolute() {
    let _guard = env_guard();
    let saved = CaseEnv::capture();
    let fx = fixture();
    let workspace = tempfile::tempdir().expect("tempdir");
    let root = workspace.path().to_path_buf();

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        build_layout(&root, &fx);
        std::env::set_var("HOME", root.join("fakehome"));
        std::env::remove_var("XDG_CONFIG_HOME");
        for name in OWNED_ENV {
            std::env::remove_var(name);
        }

        let elsewhere = root.join("elsewhere");
        let config_path = elsewhere.join("apcore.yaml");
        std::env::set_var("APCORE_CONFIG_FILE", &config_path);
        let base = elsewhere.display();
        std::fs::write(
            &config_path,
            format!(
                "version: '1.0.0'\n\
                 project: {{name: fixture}}\n\
                 extensions:\n  root: {base}/extensions\n\
                 schema:\n  root: {base}/schemas\n\
                 acl:\n  root: {base}/acl\n\
                 bindings:\n  dir: {base}/bindings\n"
            ),
        )
        .expect("write config");

        std::env::set_current_dir(root.join("project")).expect("chdir");
        let mut loaded = None;
        let logs = capture_logs(|| {
            loaded = Some(Config::discover().expect("discovery must not error"));
        });
        let config = loaded.expect("discovery produced a config");
        let cwd = std::env::current_dir().expect("cwd");

        assert_ne!(
            canon(&config.project_root()),
            canon(&cwd),
            "precondition: the project root must differ from CWD, or the case is vacuous"
        );
        assert!(
            relative_path_typed_keys(&config).is_empty(),
            "precondition: no path-typed value may remain relative, got {:?}",
            relative_path_typed_keys(&config)
        );
        assert!(
            !logs.contains("DEPRECATION"),
            "absolute values cannot re-root, so there is nothing to warn about:\n{logs}"
        );
    }));

    saved.restore();
    drop(workspace);
    if let Err(payload) = outcome {
        std::panic::resume_unwind(payload);
    }
}

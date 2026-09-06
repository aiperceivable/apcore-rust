use std::io::Write;
use std::sync::{Mutex, MutexGuard, PoisonError};
use tempfile::TempDir;

/// Serialises the tests in this file against each other.
///
/// Every test here mutates process-global state — `APCORE_CONFIG_FILE`,
/// `APCORE_BINDINGS_DIR`, the current working directory — and Cargo runs the
/// tests within one binary on parallel threads that share it. This file is
/// already a separate `[[test]]` binary (see `Cargo.toml`) so it cannot
/// cross-pollute `tests/it.rs`, but that isolation stops at the process
/// boundary and said nothing about its own tests.
///
/// The observable failure was `test_declared_env_override_still_reaches_the_
/// declared_document` setting `APCORE_BINDINGS_DIR` while the two
/// `…_is_not_a_config_override_*` tests were loading a config, which then saw a
/// `bindings.dir` key their file never declared and failed on the exact-set
/// assertion. Every test takes this guard for its whole body, so the variables
/// are set and removed with no other test in flight.
///
/// Poisoning is ignored: a panicking test leaves the lock poisoned, and the
/// remaining tests should report their own results rather than a cascade of
/// unrelated `PoisonError`s.
static ENV_GUARD: Mutex<()> = Mutex::new(());

fn env_guard() -> MutexGuard<'static, ()> {
    ENV_GUARD.lock().unwrap_or_else(PoisonError::into_inner)
}

fn write_valid_yaml(dir: &TempDir, filename: &str) -> std::path::PathBuf {
    let path = dir.path().join(filename);
    let mut f = std::fs::File::create(&path).unwrap();
    // PROTOCOL_SPEC §9.1 canonical nested namespace form (v0.18.0+).
    // Includes the spec-mandated required fields (A-D-03) so legacy-mode
    // validate() passes: version, project.name, extensions.root, schema.root,
    // acl.root, acl.default_effect.
    writeln!(f, "version: '0.15.0'").unwrap();
    writeln!(f, "project:").unwrap();
    writeln!(f, "  name: demo").unwrap();
    writeln!(f, "extensions:").unwrap();
    writeln!(f, "  root: ./extensions").unwrap();
    writeln!(f, "schema:").unwrap();
    writeln!(f, "  root: ./schemas").unwrap();
    writeln!(f, "acl:").unwrap();
    writeln!(f, "  root: ./acl").unwrap();
    writeln!(f, "  default_effect: deny").unwrap();
    writeln!(f, "executor:").unwrap();
    writeln!(f, "  max_call_depth: 32").unwrap();
    writeln!(f, "  max_module_repeat: 3").unwrap();
    writeln!(f, "  default_timeout: 30000").unwrap();
    writeln!(f, "  global_timeout: 60000").unwrap();
    path
}

#[test]
fn test_discover_uses_apcore_config_file_env_var() {
    let _guard = env_guard();
    let dir = TempDir::new().unwrap();
    let config_path = write_valid_yaml(&dir, "custom.yaml");

    // Set env var, then call discover
    std::env::set_var("APCORE_CONFIG_FILE", config_path.to_str().unwrap());
    let result = apcore::Config::discover();
    std::env::remove_var("APCORE_CONFIG_FILE");

    assert!(result.is_ok(), "discover() failed: {:?}", result.err());
}

#[test]
fn test_discover_falls_back_to_defaults_when_no_file_found() {
    let _guard = env_guard();
    // Make sure env var is not set
    std::env::remove_var("APCORE_CONFIG_FILE");

    // Run from a temp directory with no config files
    let dir = TempDir::new().unwrap();
    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();

    let result = apcore::Config::discover();

    std::env::set_current_dir(original).unwrap();

    assert!(
        result.is_ok(),
        "discover() should fall back to defaults, got: {:?}",
        result.err()
    );
    let config = result.unwrap();
    // Defaults: executor.max_call_depth = 32
    assert_eq!(config.executor.max_call_depth, 32);
}

// ---------------------------------------------------------------------------
// apcore#88: `$APCORE_CONFIG_FILE` selects the document, it is not in it.
// ---------------------------------------------------------------------------

/// Flatten the loaded settings tree into dot-paths.
fn flatten(value: &serde_json::Value, prefix: &str, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) if !map.is_empty() => {
            for (key, child) in map {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                flatten(child, &path, out);
            }
        }
        _ => out.push(prefix.to_string()),
    }
}

/// The declared document as dot-paths: the raw tree the file (plus env
/// overrides) put there. The typed `executor` / `observability` struct fields
/// are excluded by construction — they are declared by the schemas and always
/// resolve, so they say nothing about what the *document* declared.
fn declared_paths(config: &apcore::Config) -> Vec<String> {
    let mut out = Vec::new();
    for (name, value) in &config.user_namespaces {
        flatten(value, name, &mut out);
    }
    out.sort();
    out
}

fn write_minimal_yaml(dir: &TempDir, filename: &str, body: &str) -> std::path::PathBuf {
    let path = dir.path().join(filename);
    std::fs::write(&path, body).unwrap();
    path
}

/// §9.2 turns every `APCORE_*` variable into a configuration override, so the
/// file selector used to lower to the dot-path `config.file` and land in the
/// declared document — the view §9.1's required-field check runs against via
/// `get_declared`. `config.file` is declared by no schema
/// (`conformance/fixtures/config_key_governance.json`).
///
/// Asserts the **exact** declared key set, not merely that `config.file` is
/// gone: absence alone would also hold for an implementation that dropped a key
/// the file really does declare.
#[test]
fn test_config_file_env_var_is_not_a_config_override_legacy_mode() {
    let _guard = env_guard();
    let dir = TempDir::new().unwrap();
    let path = write_minimal_yaml(
        &dir,
        "custom.yaml",
        "version: '1.0.0'\nproject:\n  name: demo\n",
    );

    std::env::set_var("APCORE_CONFIG_FILE", path.to_str().unwrap());
    let config = apcore::Config::load(&path).unwrap();
    std::env::remove_var("APCORE_CONFIG_FILE");

    assert_eq!(declared_paths(&config), vec!["project.name", "version"]);
}

#[test]
fn test_config_file_env_var_is_not_a_config_override_namespace_mode() {
    let _guard = env_guard();
    let dir = TempDir::new().unwrap();
    let path = write_minimal_yaml(
        &dir,
        "ns.yaml",
        "apcore:\n  version: '1.0.0'\n  project:\n    name: demo\n",
    );

    std::env::set_var("APCORE_CONFIG_FILE", path.to_str().unwrap());
    let config = apcore::Config::load(&path).unwrap();
    std::env::remove_var("APCORE_CONFIG_FILE");

    // `version` / `project.name` appear twice: namespace-mode load mirrors the
    // LEGACY_ROOT_FIELDS to the top level for backward compatibility. That is
    // pre-existing and unrelated — what matters is that the set is closed.
    assert_eq!(
        declared_paths(&config),
        vec![
            "apcore.project.name",
            "apcore.version",
            "project.name",
            "version"
        ]
    );
}

/// The exemption is one variable wide: `bindings.dir` IS a declared key, so
/// `APCORE_BINDINGS_DIR` is §9.2 working as designed.
#[test]
fn test_declared_env_override_still_reaches_the_declared_document() {
    let _guard = env_guard();
    let dir = TempDir::new().unwrap();
    let path = write_minimal_yaml(
        &dir,
        "custom.yaml",
        "version: '1.0.0'\nproject:\n  name: demo\n",
    );

    std::env::set_var("APCORE_CONFIG_FILE", path.to_str().unwrap());
    std::env::set_var("APCORE_BINDINGS_DIR", "./generated");
    let config = apcore::Config::load(&path).unwrap();
    std::env::remove_var("APCORE_CONFIG_FILE");
    std::env::remove_var("APCORE_BINDINGS_DIR");

    assert_eq!(
        declared_paths(&config),
        vec!["bindings.dir", "project.name", "version"]
    );
}

// ---------------------------------------------------------------------------
// Config::project_root — one case per §9.14 discovery tier
// (aiperceivable/apcore#113, Option B-prime; spec §9.2.2)
// ---------------------------------------------------------------------------
//
// The tier is what selects the base, so a single "it returns a directory"
// assertion proves nothing. Each case below puts the config file and the
// process working directory in *different* places, so exactly one of the two
// candidate answers can pass. These live in this binary rather than in-file
// because they mutate CWD and `$HOME`, which only this file serialises.

/// Restores the working directory and `$HOME` on unwind as well as on success,
/// so one failing assertion cannot leave the rest of the binary running in the
/// wrong directory.
struct ProcessStateGuard {
    cwd: std::path::PathBuf,
    home: Option<String>,
}

impl ProcessStateGuard {
    fn capture() -> Self {
        Self {
            cwd: std::env::current_dir().unwrap(),
            home: std::env::var("HOME").ok(),
        }
    }
}

impl Drop for ProcessStateGuard {
    fn drop(&mut self) {
        std::env::set_current_dir(&self.cwd).unwrap();
        match &self.home {
            Some(home) => std::env::set_var("HOME", home),
            None => std::env::remove_var("HOME"),
        }
    }
}

fn canonical(path: &std::path::Path) -> std::path::PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// The tier 6 (XDG-style user-level) location under `home`, per platform.
fn tier_6_config_path(home: &std::path::Path) -> std::path::PathBuf {
    #[cfg(target_os = "macos")]
    let dir = home
        .join("Library")
        .join("Application Support")
        .join("apcore");
    #[cfg(not(target_os = "macos"))]
    let dir = home.join(".config").join("apcore");
    dir.join("config.yaml")
}

fn write_valid_yaml_at(path: &std::path::Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut f = std::fs::File::create(path).unwrap();
    writeln!(f, "version: '0.15.0'").unwrap();
    writeln!(f, "project:").unwrap();
    writeln!(f, "  name: demo").unwrap();
    writeln!(f, "extensions:").unwrap();
    writeln!(f, "  root: ./extensions").unwrap();
    writeln!(f, "schema:").unwrap();
    writeln!(f, "  root: ./schemas").unwrap();
    writeln!(f, "acl:").unwrap();
    writeln!(f, "  root: ./acl").unwrap();
    writeln!(f, "  default_effect: deny").unwrap();
}

#[derive(Clone, Default)]
struct CaptureWriter(std::sync::Arc<Mutex<Vec<u8>>>);

impl std::io::Write for CaptureWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
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

fn capture_logs<T>(f: impl FnOnce() -> T) -> (T, String) {
    let buf = CaptureWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(buf.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .finish();
    let out = tracing::subscriber::with_default(subscriber, f);
    let bytes = buf.0.lock().unwrap().clone();
    (out, String::from_utf8_lossy(&bytes).into_owned())
}

/// Tier 1 — `$APCORE_CONFIG_FILE` pointing outside CWD. The one tier where
/// file-relative and CWD-relative genuinely disagree, and the only tier B-prime
/// changes for anybody.
#[test]
fn project_root_tier_1_explicit_config_file_anchors_at_the_config_directory() {
    let _guard = env_guard();
    let _state = ProcessStateGuard::capture();

    let config_dir = TempDir::new().unwrap();
    let run_dir = TempDir::new().unwrap();
    let config_path = config_dir.path().join("custom.yaml");
    write_valid_yaml_at(&config_path);

    std::env::set_current_dir(run_dir.path()).unwrap();
    std::env::set_var("APCORE_CONFIG_FILE", config_path.to_str().unwrap());
    let (config, logs) = capture_logs(|| apcore::Config::discover().unwrap());
    std::env::remove_var("APCORE_CONFIG_FILE");

    assert_eq!(
        canonical(&config.project_root()),
        canonical(config_dir.path()),
        "an explicitly pointed-at config anchors at its own directory"
    );
    assert_ne!(
        canonical(&config.project_root()),
        canonical(run_dir.path()),
        "and not at the process working directory"
    );
    assert!(
        logs.contains("DEPRECATION"),
        "this is the tier whose relative paths re-root, so it must warn: {logs}"
    );
}

/// Tiers 2-5 — a project-local config. Its directory already *is* CWD, so both
/// candidate bases agree and nothing about this tier changes.
#[test]
fn project_root_tiers_2_to_5_project_local_config_anchors_at_cwd() {
    let _guard = env_guard();
    let _state = ProcessStateGuard::capture();
    std::env::remove_var("APCORE_CONFIG_FILE");

    for name in ["project.yaml", "project.yml", "apcore.yaml", "apcore.yml"] {
        let project = TempDir::new().unwrap();
        write_valid_yaml_at(&project.path().join(name));
        std::env::set_current_dir(project.path()).unwrap();

        let config = apcore::Config::discover().unwrap();
        assert_eq!(
            canonical(&config.project_root()),
            canonical(project.path()),
            "tier for {name} must anchor at CWD"
        );
        assert_eq!(
            config.project_root(),
            std::env::current_dir().unwrap(),
            "tier for {name} must anchor at CWD"
        );
    }
}

/// Tiers 2-5 are the majority case and nothing changes for them, so the
/// deprecation notice must stay quiet. A warning fired here would be the
/// blanket warning #113 rules out.
#[test]
fn project_root_tier_2_config_emits_no_deprecation_warning() {
    let _guard = env_guard();
    let _state = ProcessStateGuard::capture();
    std::env::remove_var("APCORE_CONFIG_FILE");

    let project = TempDir::new().unwrap();
    write_valid_yaml_at(&project.path().join("apcore.yaml"));
    std::env::set_current_dir(project.path()).unwrap();

    let (config, logs) = capture_logs(|| apcore::Config::discover().unwrap());

    assert_eq!(config.project_root(), std::env::current_dir().unwrap());
    assert!(
        !logs.contains("DEPRECATION"),
        "the ordinary project-local case must not be warned about: {logs}"
    );
}

/// Tier 6 — the user-level config, and the case #113 shows is wrong today.
/// A per-user config's relative paths are per-*project*, so the base is CWD;
/// anchoring at the config file would put `./acl` inside the user's home and
/// hand every project the same ACL policy.
#[test]
fn project_root_tier_6_user_level_config_anchors_at_cwd_not_at_home() {
    let _guard = env_guard();
    let _state = ProcessStateGuard::capture();
    std::env::remove_var("APCORE_CONFIG_FILE");

    let fake_home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    let user_config = tier_6_config_path(fake_home.path());
    write_valid_yaml_at(&user_config);

    std::env::set_var("HOME", fake_home.path().to_str().unwrap());
    std::env::set_current_dir(project.path()).unwrap();

    let config = apcore::Config::discover().unwrap();

    assert_eq!(
        canonical(config.source_path().unwrap()),
        canonical(&user_config),
        "precondition: discovery must have reached tier 6"
    );
    assert_eq!(
        canonical(&config.project_root()),
        canonical(project.path()),
        "a user-level config anchors at the project being run, not at itself"
    );
    assert_ne!(
        canonical(&config.project_root()),
        canonical(user_config.parent().unwrap()),
        "anchoring at the user-level config's own directory is the #113 defect"
    );
}

/// Tier 7 — the legacy `~/.apcore/` user-level location, same rule as tier 6.
#[test]
fn project_root_tier_7_legacy_user_level_config_anchors_at_cwd() {
    let _guard = env_guard();
    let _state = ProcessStateGuard::capture();
    std::env::remove_var("APCORE_CONFIG_FILE");

    let fake_home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    let user_config = fake_home.path().join(".apcore").join("config.yaml");
    write_valid_yaml_at(&user_config);

    std::env::set_var("HOME", fake_home.path().to_str().unwrap());
    std::env::set_current_dir(project.path()).unwrap();

    let config = apcore::Config::discover().unwrap();

    assert_eq!(
        canonical(config.source_path().unwrap()),
        canonical(&user_config),
        "precondition: discovery must have reached tier 7"
    );
    assert_eq!(canonical(&config.project_root()), canonical(project.path()),);
}

/// No config file at all — nothing to anchor to but CWD.
#[test]
fn project_root_with_no_config_file_found_anchors_at_cwd() {
    let _guard = env_guard();
    let _state = ProcessStateGuard::capture();
    std::env::remove_var("APCORE_CONFIG_FILE");

    let fake_home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    std::env::set_var("HOME", fake_home.path().to_str().unwrap());
    std::env::set_current_dir(project.path()).unwrap();

    let config = apcore::Config::discover().unwrap();

    assert_eq!(
        config.source_path(),
        None,
        "precondition: no file discovered"
    );
    assert_eq!(config.project_root(), std::env::current_dir().unwrap());
}

/// Behaviour, as opposed to the accessor, MUST NOT move in this phase (§13.2).
/// `SchemaLoader::with_config` still resolves `schema.root` against CWD and
/// `ACL::discover` still resolves `acl.root` against the config file's
/// directory, including for the user-level tier where #113 calls that wrong.
#[test]
fn tier_1_resolution_behaviour_is_unchanged_by_the_project_root_accessor() {
    let _guard = env_guard();
    let _state = ProcessStateGuard::capture();

    let config_dir = TempDir::new().unwrap();
    let run_dir = TempDir::new().unwrap();
    let config_path = config_dir.path().join("custom.yaml");
    write_valid_yaml_at(&config_path);

    // A same-named target directory under BOTH candidate bases, so exactly one
    // semantics can pass.
    std::fs::create_dir_all(config_dir.path().join("acl")).unwrap();
    std::fs::create_dir_all(run_dir.path().join("acl")).unwrap();
    std::fs::write(
        config_dir.path().join("acl").join("global_acl.yaml"),
        "version: '1.0'\ndefault_effect: deny\nrules: []\n",
    )
    .unwrap();
    std::fs::write(
        run_dir.path().join("acl").join("global_acl.yaml"),
        "version: '1.0'\ndefault_effect: allow\nrules: []\n",
    )
    .unwrap();

    // Likewise for `schema.root`, whose default `./schemas` exists under both.
    for (dir, marker) in [
        (config_dir.path(), "config_dir_marker"),
        (run_dir.path(), "run_dir_marker"),
    ] {
        std::fs::create_dir_all(dir.join("schemas")).unwrap();
        std::fs::write(
            dir.join("schemas").join("probe.schema.yaml"),
            format!(
                "module_id: probe\ndescription: probe schema\ninput_schema:\n  type: object\n  properties:\n    {marker}:\n      type: string\noutput_schema:\n  type: object\n"
            ),
        )
        .unwrap();
    }

    std::env::set_current_dir(run_dir.path()).unwrap();
    std::env::set_var("APCORE_CONFIG_FILE", config_path.to_str().unwrap());
    let config = apcore::Config::discover().unwrap();
    std::env::remove_var("APCORE_CONFIG_FILE");

    let mut loader = apcore::SchemaLoader::with_config(&config, None);
    loader.load("probe").unwrap();
    let raw = loader.get("probe").unwrap();
    assert!(
        raw["input_schema"]["properties"]
            .get("run_dir_marker")
            .is_some(),
        "schema.root must still resolve against the process working directory: {raw}"
    );

    let acl = apcore::ACL::discover(&config).unwrap();
    assert!(
        acl.is_some(),
        "acl.root must still resolve against the config file's directory"
    );
}

use std::io::Write;
use tempfile::TempDir;

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

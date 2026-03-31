use std::io::Write;
use tempfile::TempDir;

fn write_valid_yaml(dir: &TempDir, filename: &str) -> std::path::PathBuf {
    let path = dir.path().join(filename);
    let mut f = std::fs::File::create(&path).unwrap();
    writeln!(f, "max_call_depth: 32").unwrap();
    writeln!(f, "max_module_repeat: 3").unwrap();
    writeln!(f, "default_timeout_ms: 30000").unwrap();
    writeln!(f, "global_timeout_ms: 60000").unwrap();
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
    // Defaults: max_call_depth = 32
    assert_eq!(config.max_call_depth, 32);
}

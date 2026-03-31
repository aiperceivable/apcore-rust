// Integration tests for Config Bus (§9.4–§9.15, v0.15.0)

use apcore::config::{Config, ConfigMode, MountSource, NamespaceRegistration};
use apcore::errors::ErrorCode;
use serde::Deserialize;

// ---------------------------------------------------------------------------
// Namespace registration
// ---------------------------------------------------------------------------

#[test]
fn test_register_namespace_reserved_name_returns_error() {
    let result = Config::register_namespace(NamespaceRegistration {
        name: "apcore".to_string(),
        env_prefix: None,
        defaults: None,
        schema: None,
    });
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().code, ErrorCode::ConfigNamespaceReserved);
}

#[test]
fn test_register_namespace_reserved_config_name_returns_error() {
    let result = Config::register_namespace(NamespaceRegistration {
        name: "_config".to_string(),
        env_prefix: None,
        defaults: None,
        schema: None,
    });
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().code, ErrorCode::ConfigNamespaceReserved);
}

#[test]
fn test_register_namespace_env_prefix_duplicate_raises() {
    // Duplicate env_prefix should raise ConfigEnvPrefixConflict.
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let prefix = format!("DUP_PREFIX_{ts}");
    Config::register_namespace(NamespaceRegistration {
        name: format!("dup_pfx_a_{ts}"),
        env_prefix: Some(prefix.clone()),
        defaults: None,
        schema: None,
    })
    .unwrap();
    let result = Config::register_namespace(NamespaceRegistration {
        name: format!("dup_pfx_b_{ts}"),
        env_prefix: Some(prefix),
        defaults: None,
        schema: None,
    });
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().code, ErrorCode::ConfigEnvPrefixConflict);
}

#[test]
fn test_register_namespace_success_and_list() {
    let name = format!(
        "test_ns_list_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    );

    Config::register_namespace(NamespaceRegistration {
        name: name.clone(),
        env_prefix: None,
        defaults: Some(serde_json::json!({"key": "value"})),
        schema: None,
    })
    .unwrap();

    let namespaces = Config::registered_namespaces();
    let found = namespaces.iter().any(|n| n.name == name);
    assert!(found, "registered namespace must appear in list");
}

#[test]
fn test_register_namespace_duplicate_returns_error() {
    let name = format!(
        "test_dup_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    );

    Config::register_namespace(NamespaceRegistration {
        name: name.clone(),
        env_prefix: None,
        defaults: None,
        schema: None,
    })
    .unwrap();

    let result = Config::register_namespace(NamespaceRegistration {
        name: name.clone(),
        env_prefix: None,
        defaults: None,
        schema: None,
    });
    assert!(result.is_err());
    assert_eq!(
        result.unwrap_err().code,
        ErrorCode::ConfigNamespaceDuplicate
    );
}

// ---------------------------------------------------------------------------
// Mode detection
// ---------------------------------------------------------------------------

#[test]
fn test_mode_detection_legacy_from_defaults() {
    let config = Config::from_defaults();
    // from_defaults() without an "apcore" top-level key → Legacy mode.
    assert_eq!(config.mode, ConfigMode::Legacy);
}

// ---------------------------------------------------------------------------
// Namespace get / set
// ---------------------------------------------------------------------------

#[test]
fn test_namespace_mode_get_set_dot_path() {
    let mut config = Config::from_defaults();
    // Force namespace mode so we can exercise namespace path handling.
    config.mode = ConfigMode::Namespace;
    config
        .settings
        .insert("apcore".to_string(), serde_json::json!({}));

    config.set(
        "my-adapter.transport",
        serde_json::Value::String("grpc".to_string()),
    );
    let val = config.get("my-adapter.transport");
    assert_eq!(val, Some(serde_json::Value::String("grpc".to_string())));
}

#[test]
fn test_namespace_method_returns_subtree() {
    let mut config = Config::from_defaults();
    config.mode = ConfigMode::Namespace;
    config.settings.insert(
        "widget".to_string(),
        serde_json::json!({"color": "blue", "size": 42}),
    );

    let ns = config.namespace("widget").unwrap();
    assert_eq!(ns["color"], serde_json::json!("blue"));
    assert_eq!(ns["size"], serde_json::json!(42));
}

#[test]
fn test_namespace_method_returns_none_for_unknown() {
    let config = Config::from_defaults();
    assert!(config.namespace("nonexistent_ns_xyz").is_none());
}

// ---------------------------------------------------------------------------
// Mount
// ---------------------------------------------------------------------------

#[test]
fn test_mount_dict_merges_into_namespace() {
    let mut config = Config::from_defaults();
    config.mode = ConfigMode::Namespace;
    config.settings.insert(
        "plugins".to_string(),
        serde_json::json!({"existing_key": true}),
    );

    config
        .mount(
            "plugins",
            MountSource::Dict(serde_json::json!({"new_key": "hello"})),
        )
        .unwrap();

    let ns = config.namespace("plugins").unwrap();
    assert_eq!(ns["existing_key"], serde_json::json!(true));
    assert_eq!(ns["new_key"], serde_json::json!("hello"));
}

#[test]
fn test_mount_non_object_returns_error() {
    let mut config = Config::from_defaults();
    let result = config.mount("plugins", MountSource::Dict(serde_json::json!([1, 2, 3])));
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().code, ErrorCode::ConfigMountError);
}

// ---------------------------------------------------------------------------
// bind / get_typed
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, PartialEq)]
struct PluginConfig {
    enabled: bool,
    max_workers: u32,
}

#[test]
fn test_bind_namespace_into_typed_struct() {
    let mut config = Config::from_defaults();
    config.mode = ConfigMode::Namespace;
    config.settings.insert(
        "plugin_bind_test".to_string(),
        serde_json::json!({"enabled": true, "max_workers": 4}),
    );

    let result: PluginConfig = config.bind("plugin_bind_test").unwrap();
    assert!(result.enabled);
    assert_eq!(result.max_workers, 4);
}

#[test]
fn test_bind_unknown_namespace_returns_error() {
    let config = Config::from_defaults();
    let result: Result<PluginConfig, _> = config.bind("totally_unknown_ns");
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().code, ErrorCode::ConfigBindError);
}

#[test]
fn test_get_typed_returns_value() {
    let mut config = Config::from_defaults();
    config.mode = ConfigMode::Namespace;
    config
        .settings
        .insert("svc".to_string(), serde_json::json!({"port": 8080}));

    let port: u64 = config.get_typed("svc.port").unwrap();
    assert_eq!(port, 8080);
}

// ---------------------------------------------------------------------------
// Built-in namespaces (§9.15)
// ---------------------------------------------------------------------------

#[test]
fn test_builtin_namespaces_observability_registered() {
    // from_defaults() triggers init_builtin_namespaces().
    let _ = Config::from_defaults();
    let ns = Config::registered_namespaces();
    let has_observability = ns.iter().any(|n| n.name == "observability");
    assert!(
        has_observability,
        "observability namespace must be built-in"
    );
}

#[test]
fn test_builtin_namespaces_sys_modules_registered() {
    let _ = Config::from_defaults();
    let ns = Config::registered_namespaces();
    let has_sys = ns.iter().any(|n| n.name == "sys_modules");
    assert!(has_sys, "sys_modules namespace must be built-in");
}

// Integration tests for Config Bus (§9.4–§9.15, v0.15.0)

use std::collections::HashMap;

use apcore::config::{
    Config, ConfigMode, EnvStyle, MountSource, NamespaceRegistration, DEFAULT_MAX_DEPTH,
};
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
        env_style: EnvStyle::Nested,
        max_depth: DEFAULT_MAX_DEPTH,
        env_map: None,
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
        env_style: EnvStyle::Nested,
        max_depth: DEFAULT_MAX_DEPTH,
        env_map: None,
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
        env_style: EnvStyle::Nested,
        max_depth: DEFAULT_MAX_DEPTH,
        env_map: None,
    })
    .unwrap();
    let result = Config::register_namespace(NamespaceRegistration {
        name: format!("dup_pfx_b_{ts}"),
        env_prefix: Some(prefix),
        defaults: None,
        schema: None,
        env_style: EnvStyle::Nested,
        max_depth: DEFAULT_MAX_DEPTH,
        env_map: None,
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
        env_style: EnvStyle::Nested,
        max_depth: DEFAULT_MAX_DEPTH,
        env_map: None,
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
        env_style: EnvStyle::Nested,
        max_depth: DEFAULT_MAX_DEPTH,
        env_map: None,
    })
    .unwrap();

    let result = Config::register_namespace(NamespaceRegistration {
        name: name.clone(),
        env_prefix: None,
        defaults: None,
        schema: None,
        env_style: EnvStyle::Nested,
        max_depth: DEFAULT_MAX_DEPTH,
        env_map: None,
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
        .user_namespaces
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
    config.user_namespaces.insert(
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
    config.user_namespaces.insert(
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
    config.user_namespaces.insert(
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
        .user_namespaces
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

// ---------------------------------------------------------------------------
// env_style flat
// ---------------------------------------------------------------------------

#[test]
fn test_env_style_flat_preserves_underscores() {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let ns_name = format!("flatns-{ts}");
    let prefix = format!("FLAT_{ts}");

    Config::register_namespace(NamespaceRegistration {
        name: ns_name.clone(),
        env_prefix: Some(prefix.clone()),
        defaults: None,
        schema: None,
        env_style: EnvStyle::Flat,
        max_depth: DEFAULT_MAX_DEPTH,
        env_map: None,
    })
    .unwrap();

    // Set env vars with underscores in the key name.
    let key1 = format!("{prefix}_DEVTO_API_KEY");
    let key2 = format!("{prefix}_LLM_MODEL");
    std::env::set_var(&key1, "abc123");
    std::env::set_var(&key2, "gemini-pro");

    // Load from a namespace-mode YAML file so env overrides are applied.
    let tmp_dir = std::env::temp_dir().join(format!("apcore-flat-test-{ts}"));
    std::fs::create_dir_all(&tmp_dir).unwrap();
    let yaml_path = tmp_dir.join("cfg.yaml");
    std::fs::write(
        &yaml_path,
        "executor:\n  max_call_depth: 32\n  max_module_repeat: 3\n  default_timeout: 30000\n  global_timeout: 60000\napcore:\n  version: '1.0.0'\n",
    )
    .unwrap();
    let config = Config::load(&yaml_path).unwrap();

    // Flat style: underscores preserved, no nesting.
    assert_eq!(
        config.get(&format!("{ns_name}.devto_api_key")),
        Some(serde_json::json!("abc123"))
    );
    assert_eq!(
        config.get(&format!("{ns_name}.llm_model")),
        Some(serde_json::json!("gemini-pro"))
    );

    // Cleanup.
    std::env::remove_var(&key1);
    std::env::remove_var(&key2);
    let _ = std::fs::remove_dir_all(&tmp_dir);
}

const NS_YAML: &str = "executor:\n  max_call_depth: 32\n  max_module_repeat: 3\n  default_timeout: 30000\n  global_timeout: 60000\napcore:\n  version: '1.0.0'\n";

#[test]
fn test_env_style_auto_resolves_mixed_flat_and_nested() {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let ns_name = format!("autons-{ts}");
    let prefix = format!("AUTO_{ts}");

    Config::register_namespace(NamespaceRegistration {
        name: ns_name.clone(),
        env_prefix: Some(prefix.clone()),
        defaults: Some(serde_json::json!({
            "devto_api_key": "",
            "publish": { "delay": 5, "retry": 3 }
        })),
        schema: None,
        env_style: EnvStyle::Auto,
        max_depth: DEFAULT_MAX_DEPTH,
        env_map: None,
    })
    .unwrap();

    let k1 = format!("{prefix}_DEVTO_API_KEY");
    let k2 = format!("{prefix}_PUBLISH_DELAY");
    let k3 = format!("{prefix}_PUBLISH_RETRY");
    std::env::set_var(&k1, "abc123");
    std::env::set_var(&k2, "10");
    std::env::set_var(&k3, "7");

    let tmp_dir = std::env::temp_dir().join(format!("apcore-auto-test-{ts}"));
    std::fs::create_dir_all(&tmp_dir).unwrap();
    let yaml_path = tmp_dir.join("cfg.yaml");
    std::fs::write(&yaml_path, NS_YAML).unwrap();
    let config = Config::load(&yaml_path).unwrap();

    // Flat key matched.
    assert_eq!(
        config.get(&format!("{ns_name}.devto_api_key")),
        Some(serde_json::json!("abc123"))
    );
    // Nested keys matched.
    assert_eq!(
        config.get(&format!("{ns_name}.publish.delay")),
        Some(serde_json::json!(10))
    );
    assert_eq!(
        config.get(&format!("{ns_name}.publish.retry")),
        Some(serde_json::json!(7))
    );

    std::env::remove_var(&k1);
    std::env::remove_var(&k2);
    std::env::remove_var(&k3);
    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn test_max_depth_limits_nesting() {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let ns_name = format!("depthns-{ts}");
    let prefix = format!("DEPTH_{ts}");

    Config::register_namespace(NamespaceRegistration {
        name: ns_name.clone(),
        env_prefix: Some(prefix.clone()),
        defaults: None,
        schema: None,
        env_style: EnvStyle::Nested,
        max_depth: 3,
        env_map: None,
    })
    .unwrap();

    let k1 = format!("{prefix}_A_B_C_D_E");
    std::env::set_var(&k1, "val");

    let tmp_dir = std::env::temp_dir().join(format!("apcore-depth-test-{ts}"));
    std::fs::create_dir_all(&tmp_dir).unwrap();
    let yaml_path = tmp_dir.join("cfg.yaml");
    std::fs::write(&yaml_path, NS_YAML).unwrap();
    let config = Config::load(&yaml_path).unwrap();

    // max_depth=3: at most 3 segments (2 dots), rest literal.
    assert_eq!(
        config.get(&format!("{ns_name}.a.b.c_d_e")),
        Some(serde_json::json!("val"))
    );

    std::env::remove_var(&k1);
    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn test_env_prefix_auto_derived_from_name() {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let ns_name = format!("autoderiv-{ts}");
    // No env_prefix → auto-derived as "AUTODERIV_{ts}" (uppercase, - → _)
    Config::register_namespace(NamespaceRegistration {
        name: ns_name.clone(),
        env_prefix: None, // auto-derive
        defaults: None,
        schema: None,
        env_style: EnvStyle::Auto,
        max_depth: DEFAULT_MAX_DEPTH,
        env_map: None,
    })
    .unwrap();

    let derived_prefix = ns_name.to_uppercase().replace('-', "_");
    let k1 = format!("{derived_prefix}_FOO");
    std::env::set_var(&k1, "bar");

    let tmp_dir = std::env::temp_dir().join(format!("apcore-autoderiv-{ts}"));
    std::fs::create_dir_all(&tmp_dir).unwrap();
    let yaml_path = tmp_dir.join("cfg.yaml");
    std::fs::write(&yaml_path, NS_YAML).unwrap();
    let config = Config::load(&yaml_path).unwrap();

    assert_eq!(
        config.get(&format!("{ns_name}.foo")),
        Some(serde_json::json!("bar"))
    );

    std::env::remove_var(&k1);
    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn test_namespace_env_map() {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let ns_name = format!("envmapns-{ts}");
    let redis_key = format!("REDIS_URL_{ts}");
    Config::register_namespace(NamespaceRegistration {
        name: ns_name.clone(),
        env_prefix: None,
        defaults: None,
        schema: None,
        env_style: EnvStyle::Auto,
        max_depth: DEFAULT_MAX_DEPTH,
        env_map: Some(HashMap::from([(
            redis_key.clone(),
            "cache_url".to_string(),
        )])),
    })
    .unwrap();

    std::env::set_var(&redis_key, "redis://localhost");

    let tmp_dir = std::env::temp_dir().join(format!("apcore-envmap-{ts}"));
    std::fs::create_dir_all(&tmp_dir).unwrap();
    let yaml_path = tmp_dir.join("cfg.yaml");
    std::fs::write(&yaml_path, NS_YAML).unwrap();
    let config = Config::load(&yaml_path).unwrap();

    assert_eq!(
        config.get(&format!("{ns_name}.cache_url")),
        Some(serde_json::json!("redis://localhost"))
    );

    std::env::remove_var(&redis_key);
    let _ = std::fs::remove_dir_all(&tmp_dir);
}

// ---------------------------------------------------------------------------
// Sync CB-001 — validate() spec-required field constraints
// ---------------------------------------------------------------------------

#[test]
fn test_validate_rejects_invalid_acl_default_effect() {
    let mut cfg = Config::from_defaults();
    cfg.set("acl.default_effect", serde_json::json!("maybe"));
    let err = cfg
        .validate()
        .expect_err("invalid acl.default_effect must be rejected");
    assert_eq!(err.code, ErrorCode::ConfigInvalid);
    assert!(
        err.message.contains("acl.default_effect"),
        "error must mention acl.default_effect: {}",
        err.message
    );
}

#[test]
fn test_validate_accepts_allow_or_deny_for_default_effect() {
    let mut cfg = Config::from_defaults();
    cfg.set("acl.default_effect", serde_json::json!("allow"));
    cfg.validate().unwrap();
    cfg.set("acl.default_effect", serde_json::json!("deny"));
    cfg.validate().unwrap();
}

#[test]
fn test_validate_rejects_sampling_rate_out_of_range() {
    let mut cfg = Config::from_defaults();
    cfg.set(
        "observability.tracing.sampling_rate",
        serde_json::json!(1.5),
    );
    let err = cfg
        .validate()
        .expect_err("sampling_rate > 1.0 must be rejected");
    assert!(err.message.contains("sampling_rate"));
}

#[test]
fn test_validate_accepts_sampling_rate_in_unit_interval() {
    let mut cfg = Config::from_defaults();
    cfg.set(
        "observability.tracing.sampling_rate",
        serde_json::json!(0.0),
    );
    cfg.validate().unwrap();
    cfg.set(
        "observability.tracing.sampling_rate",
        serde_json::json!(0.5),
    );
    cfg.validate().unwrap();
    cfg.set(
        "observability.tracing.sampling_rate",
        serde_json::json!(1.0),
    );
    cfg.validate().unwrap();
}

// ---------------------------------------------------------------------------
// Sync CB-002 — mount() deep-merge
// ---------------------------------------------------------------------------

#[test]
fn test_mount_deep_merges_nested_objects() {
    let mut cfg = Config::from_defaults();
    cfg.user_namespaces
        .insert("db".to_string(), serde_json::json!({"port": 5432}));
    cfg.mount("db", MountSource::Dict(serde_json::json!({"host": "a"})))
        .unwrap();
    let db = cfg.namespace("db").unwrap();
    assert_eq!(db["host"], "a");
    assert_eq!(
        db["port"], 5432,
        "peer key 'port' must be preserved by deep-merge"
    );
}

#[test]
fn test_mount_deep_merges_recursively_under_nested_keys() {
    let mut cfg = Config::from_defaults();
    cfg.user_namespaces.insert(
        "services".to_string(),
        serde_json::json!({
            "auth": { "host": "auth.local", "port": 8080 }
        }),
    );
    cfg.mount(
        "services",
        MountSource::Dict(serde_json::json!({
            "auth": { "tls": true }
        })),
    )
    .unwrap();
    let auth = cfg.namespace("services").unwrap()["auth"].clone();
    assert_eq!(auth["host"], "auth.local", "peer key preserved at depth 2");
    assert_eq!(auth["port"], 8080);
    assert_eq!(auth["tls"], true);
}

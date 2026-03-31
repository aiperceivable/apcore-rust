// APCore Protocol — Configuration
// Spec reference: Configuration loading, validation, and environment variable overrides (Algorithm A12)

use regex::Regex;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};

use crate::errors::{ErrorCode, ModuleError};

/// Configuration mode detected from YAML content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ConfigMode {
    #[default]
    Legacy,
    Namespace,
}

/// Source for a `config.mount()` operation.
pub enum MountSource {
    Dict(serde_json::Value),
    File(PathBuf),
}

/// Registration info for a Config Bus namespace.
#[derive(Debug, Clone)]
pub struct NamespaceRegistration {
    pub name: String,
    pub env_prefix: Option<String>,
    pub defaults: Option<serde_json::Value>,
    pub schema: Option<serde_json::Value>,
}

/// Summary of a registered namespace (returned by `registered_namespaces()`).
#[derive(Debug, Clone)]
pub struct NamespaceInfo {
    pub name: String,
    pub env_prefix: Option<String>,
    pub has_schema: bool,
}

static GLOBAL_NS_REGISTRY: OnceLock<RwLock<HashMap<String, NamespaceRegistration>>> =
    OnceLock::new();

fn global_ns_registry() -> &'static RwLock<HashMap<String, NamespaceRegistration>> {
    GLOBAL_NS_REGISTRY.get_or_init(|| RwLock::new(HashMap::new()))
}

const RESERVED_NAMESPACES: &[&str] = &["apcore", "_config"];

fn reserved_env_prefix_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^APCORE_[A-Z0-9]").unwrap())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub modules_path: Option<PathBuf>,
    #[serde(default)]
    pub max_call_depth: u32,
    #[serde(default)]
    pub max_module_repeat: u32,
    #[serde(default)]
    pub default_timeout_ms: u64,
    #[serde(default)]
    pub global_timeout_ms: u64,
    #[serde(default)]
    pub enable_tracing: bool,
    #[serde(default)]
    pub enable_metrics: bool,
    #[serde(default)]
    pub settings: HashMap<String, serde_json::Value>,
    #[serde(skip)]
    pub yaml_path: Option<PathBuf>,
    #[serde(skip)]
    pub mode: ConfigMode,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            modules_path: None,
            max_call_depth: 32,
            max_module_repeat: 3,
            default_timeout_ms: 30000,
            global_timeout_ms: 60000,
            enable_tracing: false,
            enable_metrics: false,
            settings: HashMap::new(),
            yaml_path: None,
            mode: ConfigMode::default(),
        }
    }
}

impl Config {
    /// Load config from a JSON file, apply env overrides, and validate.
    pub fn from_json_file(path: &std::path::Path) -> Result<Self, ModuleError> {
        let file = std::fs::File::open(path).map_err(|e| {
            ModuleError::new(
                ErrorCode::ConfigNotFound,
                format!("Config file not found: {}: {}", path.display(), e),
            )
        })?;
        let reader = std::io::BufReader::new(file);
        let mut config: Config = serde_json::from_reader(reader).map_err(|e| {
            ModuleError::new(
                ErrorCode::ConfigInvalid,
                format!("Failed to parse JSON config: {}: {}", path.display(), e),
            )
        })?;
        config.apply_env_overrides();
        config.detect_mode();
        init_builtin_namespaces();
        config.validate()?;
        Ok(config)
    }

    /// Load config from a YAML file, apply env overrides, and validate.
    pub fn from_yaml_file(path: &std::path::Path) -> Result<Self, ModuleError> {
        let file = std::fs::File::open(path).map_err(|e| {
            ModuleError::new(
                ErrorCode::ConfigNotFound,
                format!("Config file not found: {}: {}", path.display(), e),
            )
        })?;
        let reader = std::io::BufReader::new(file);
        let mut config: Config = serde_yaml::from_reader(reader).map_err(|e| {
            ModuleError::new(
                ErrorCode::ConfigInvalid,
                format!("Failed to parse YAML config: {}: {}", path.display(), e),
            )
        })?;
        config.yaml_path = Some(path.to_path_buf());
        config.apply_env_overrides();
        config.detect_mode();
        init_builtin_namespaces();
        config.validate()?;
        Ok(config)
    }

    /// Auto-detect format by file extension and load.
    pub fn load(path: &std::path::Path) -> Result<Self, ModuleError> {
        match path.extension().and_then(|e| e.to_str()) {
            Some("json") => Self::from_json_file(path),
            Some("yaml") | Some("yml") => Self::from_yaml_file(path),
            _ => {
                // Default to YAML
                Self::from_yaml_file(path)
            }
        }
    }

    /// Validate config constraints. Returns an error listing all violations.
    pub fn validate(&self) -> Result<(), ModuleError> {
        let mut errors: Vec<String> = Vec::new();

        if self.max_call_depth < 1 {
            errors.push("max_call_depth must be >= 1".to_string());
        }
        if self.max_module_repeat < 1 {
            errors.push("max_module_repeat must be >= 1".to_string());
        }
        // default_timeout_ms == 0 means no timeout, which is allowed
        // but any positive value is fine too, so no constraint needed beyond >= 0 (u64 is always >= 0)
        if self.global_timeout_ms > 0
            && self.default_timeout_ms > 0
            && self.global_timeout_ms < self.default_timeout_ms
        {
            errors.push(format!(
                "global_timeout_ms ({}) must be >= default_timeout_ms ({})",
                self.global_timeout_ms, self.default_timeout_ms
            ));
        }

        if errors.is_empty() {
            Ok(())
        } else {
            let message = format!("Config validation failed: {}", errors.join("; "));
            Err(ModuleError::new(ErrorCode::ConfigInvalid, message))
        }
    }

    /// Build config from defaults, applying env var overrides.
    pub fn from_defaults() -> Self {
        let mut config = Self::default();
        config.apply_env_overrides();
        config.detect_mode();
        init_builtin_namespaces();
        config
    }

    /// Discover and load config using the §9.14 search order.
    ///
    /// If no file is found, returns `Config::from_defaults()`.
    pub fn discover() -> Result<Self, ModuleError> {
        match discover_config_file() {
            Some(path) => Self::load(&path),
            None => Ok(Self::from_defaults()),
        }
    }

    /// Get a config value by dot-path key.
    ///
    /// First checks typed fields (e.g. "executor.max_call_depth"), then falls
    /// back to the `settings` HashMap for arbitrary nested config.
    pub fn get(&self, key: &str) -> Option<serde_json::Value> {
        // Check typed fields first
        if let Some(val) = self.get_typed_field(key) {
            return Some(val);
        }

        // Fall back to settings HashMap with dot-path traversal
        let parts: Vec<&str> = key.split('.').collect();
        if parts.is_empty() {
            return None;
        }
        let top = self.settings.get(parts[0])?;
        if parts.len() == 1 {
            return Some(top.clone());
        }
        let mut current = top;
        for part in &parts[1..] {
            current = current.get(*part)?;
        }
        Some(current.clone())
    }

    /// Set a config value by dot-path key.
    ///
    /// Attempts to set typed fields first, then falls back to the settings HashMap.
    pub fn set(&mut self, key: &str, value: serde_json::Value) {
        // Try to set a typed field
        if self.set_typed_field(key, &value) {
            return;
        }

        // Fall back to settings HashMap
        let parts: Vec<&str> = key.split('.').collect();
        if parts.is_empty() {
            return;
        }
        if parts.len() == 1 {
            self.settings.insert(key.to_string(), value);
            return;
        }
        let root = self
            .settings
            .entry(parts[0].to_string())
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        let mut current = root;
        for part in &parts[1..parts.len() - 1] {
            if !current.is_object() {
                *current = serde_json::Value::Object(serde_json::Map::new());
            }
            current = current
                .as_object_mut()
                .unwrap()
                .entry(part.to_string())
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        }
        if !current.is_object() {
            *current = serde_json::Value::Object(serde_json::Map::new());
        }
        current
            .as_object_mut()
            .unwrap()
            .insert(parts[parts.len() - 1].to_string(), value);
    }

    /// Reload config from the stored yaml_path. Returns error if no path stored.
    pub fn reload(&mut self) -> Result<(), ModuleError> {
        let path = self.yaml_path.clone().ok_or_else(|| {
            ModuleError::new(
                ErrorCode::ReloadFailed,
                "Cannot reload: no yaml_path stored (config was not loaded from a file)",
            )
        })?;
        let reloaded = Self::load(&path)?;
        // Preserve the yaml_path through reload
        let yaml_path = self.yaml_path.take();
        *self = reloaded;
        self.yaml_path = yaml_path;
        Ok(())
    }

    /// Return a `serde_json::Value` representing the full config as a nested
    /// JSON object — typed fields merged on top of the settings map.
    pub fn data(&self) -> serde_json::Value {
        // Start with settings as the base
        let mut root = serde_json::Map::new();
        for (k, v) in &self.settings {
            root.insert(k.clone(), v.clone());
        }

        // Merge typed fields under "executor" namespace
        let executor = root
            .entry("executor".to_string())
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        if let Some(obj) = executor.as_object_mut() {
            obj.insert(
                "max_call_depth".to_string(),
                serde_json::Value::Number(self.max_call_depth.into()),
            );
            obj.insert(
                "max_module_repeat".to_string(),
                serde_json::Value::Number(self.max_module_repeat.into()),
            );
            obj.insert(
                "default_timeout_ms".to_string(),
                serde_json::Value::Number(self.default_timeout_ms.into()),
            );
            obj.insert(
                "global_timeout_ms".to_string(),
                serde_json::Value::Number(self.global_timeout_ms.into()),
            );
        }

        // Merge typed fields under "observability" namespace
        let observability = root
            .entry("observability".to_string())
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        if let Some(obj) = observability.as_object_mut() {
            obj.insert(
                "enable_tracing".to_string(),
                serde_json::Value::Bool(self.enable_tracing),
            );
            obj.insert(
                "enable_metrics".to_string(),
                serde_json::Value::Bool(self.enable_metrics),
            );
        }

        // Add modules_path at the top level if set
        if let Some(ref p) = self.modules_path {
            root.insert(
                "modules_path".to_string(),
                serde_json::Value::String(p.to_string_lossy().into_owned()),
            );
        }

        serde_json::Value::Object(root)
    }

    // --- Namespace registration (class methods) ---

    pub fn register_namespace(reg: NamespaceRegistration) -> Result<(), ModuleError> {
        if RESERVED_NAMESPACES.contains(&reg.name.as_str()) {
            return Err(ModuleError::config_namespace_reserved(&reg.name));
        }
        if let Some(ref prefix) = reg.env_prefix {
            if reserved_env_prefix_pattern().is_match(prefix) {
                return Err(ModuleError::config_env_prefix_conflict(prefix));
            }
        }
        let mut map = global_ns_registry()
            .write()
            .map_err(|_| ModuleError::config_mount_error(&reg.name, "registry lock poisoned"))?;
        if map.contains_key(&reg.name) {
            return Err(ModuleError::config_namespace_duplicate(&reg.name));
        }
        map.insert(reg.name.clone(), reg);
        Ok(())
    }

    pub fn registered_namespaces() -> Vec<NamespaceInfo> {
        global_ns_registry()
            .read()
            .map(|m| {
                m.values()
                    .map(|r| NamespaceInfo {
                        name: r.name.clone(),
                        env_prefix: r.env_prefix.clone(),
                        has_schema: r.schema.is_some(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    // --- Namespace instance methods ---

    pub fn namespace(&self, name: &str) -> Option<serde_json::Value> {
        self.settings.get(name).cloned()
    }

    pub fn mount(&mut self, namespace: &str, source: MountSource) -> Result<(), ModuleError> {
        // W-2: Reject reserved namespace per §9.7 spec.
        if namespace == "_config" {
            return Err(ModuleError::config_mount_error(
                namespace,
                "cannot mount to reserved namespace '_config'",
            ));
        }
        let data = match source {
            MountSource::Dict(v) => v,
            MountSource::File(path) => {
                let content = std::fs::read_to_string(&path)
                    .map_err(|e| ModuleError::config_mount_error(namespace, &e.to_string()))?;
                serde_yaml::from_str(&content)
                    .map_err(|e| ModuleError::config_mount_error(namespace, &e.to_string()))?
            }
        };
        if !data.is_object() {
            return Err(ModuleError::config_mount_error(
                namespace,
                "mount source must be a JSON object",
            ));
        }
        let entry = self
            .settings
            .entry(namespace.to_string())
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        if let (Some(target), Some(source_map)) = (entry.as_object_mut(), data.as_object()) {
            for (k, v) in source_map {
                target.insert(k.clone(), v.clone());
            }
        }
        Ok(())
    }

    pub fn bind<T: DeserializeOwned>(&self, namespace: &str) -> Result<T, ModuleError> {
        let value = self
            .settings
            .get(namespace)
            .ok_or_else(|| ModuleError::config_bind_error(namespace, "namespace not found"))?;
        serde_json::from_value(value.clone())
            .map_err(|e| ModuleError::config_bind_error(namespace, &e.to_string()))
    }

    pub fn get_typed<T: DeserializeOwned>(&self, key: &str) -> Result<T, ModuleError> {
        let value = self
            .get(key)
            .ok_or_else(|| ModuleError::config_bind_error(key, "key not found"))?;
        serde_json::from_value(value)
            .map_err(|e| ModuleError::config_bind_error(key, &e.to_string()))
    }

    // --- Private helpers ---

    fn detect_mode(&mut self) {
        // W-3: Only activate namespace mode when "apcore" key is a mapping.
        // A null or scalar value is not a valid namespace indicator.
        self.mode = match self.settings.get("apcore") {
            Some(serde_json::Value::Object(_)) => ConfigMode::Namespace,
            _ => ConfigMode::Legacy,
        };
    }

    /// Apply APCORE_* environment variable overrides to both typed fields and settings.
    ///
    /// In legacy mode, all `APCORE_*` vars are mapped via `env_key_to_dot_path`.
    /// In namespace mode, registered `env_prefix` values are dispatched via
    /// longest-prefix-match (§9.10).
    fn apply_env_overrides(&mut self) {
        if self.mode == ConfigMode::Namespace {
            self.apply_namespace_env_overrides();
            return;
        }
        // Legacy mode: flat APCORE_ prefix stripping.
        for (key, value) in std::env::vars() {
            if let Some(suffix) = key.strip_prefix("APCORE_") {
                let dot_path = Self::env_key_to_dot_path(suffix);
                let parsed = Self::coerce_env_value(&value);
                tracing::debug!(env = %key, path = %dot_path, "Applying legacy env override");
                self.set(&dot_path, parsed);
            }
        }
    }

    /// §9.10: Namespace-aware env routing via longest-prefix-match.
    fn apply_namespace_env_overrides(&mut self) {
        let registry = global_ns_registry()
            .read()
            .unwrap_or_else(|e| e.into_inner());
        // Collect registered prefixes, sorted by length descending (longest first).
        let mut prefixed: Vec<(&str, &str)> = registry
            .values()
            .filter_map(|r| r.env_prefix.as_deref().map(|pfx| (pfx, r.name.as_str())))
            .collect();
        prefixed.sort_by(|a, b| b.0.len().cmp(&a.0.len()));

        for (env_key, env_value) in std::env::vars() {
            let parsed = Self::coerce_env_value(&env_value);
            // Try namespace-aware routing first.
            let mut matched = false;
            for &(prefix, ns_name) in &prefixed {
                if let Some(suffix) = env_key.strip_prefix(prefix) {
                    // Strip the leading separator (usually '_').
                    let suffix = suffix.strip_prefix('_').unwrap_or(suffix);
                    if suffix.is_empty() {
                        continue;
                    }
                    let dot_path = Self::env_key_to_dot_path(suffix);
                    let full_path = format!("{ns_name}.{dot_path}");
                    tracing::debug!(env = %env_key, path = %full_path, "Applying namespace env override");
                    self.set(&full_path, parsed.clone());
                    matched = true;
                    break;
                }
            }
            // Fallback: legacy APCORE_ prefix for the apcore sub-namespace.
            if !matched {
                if let Some(suffix) = env_key.strip_prefix("APCORE_") {
                    let dot_path = Self::env_key_to_dot_path(suffix);
                    tracing::debug!(env = %env_key, path = %dot_path, "Applying legacy env override in namespace mode");
                    self.set(&dot_path, parsed);
                }
            }
        }
    }

    /// Map a dot-path key to a typed field value, if it matches a known field.
    fn get_typed_field(&self, key: &str) -> Option<serde_json::Value> {
        match key {
            "executor.max_call_depth" | "max_call_depth" => {
                Some(serde_json::Value::Number(self.max_call_depth.into()))
            }
            "executor.max_module_repeat" | "max_module_repeat" => {
                Some(serde_json::Value::Number(self.max_module_repeat.into()))
            }
            "executor.default_timeout_ms" | "default_timeout_ms" => {
                Some(serde_json::Value::Number(self.default_timeout_ms.into()))
            }
            "executor.global_timeout_ms" | "global_timeout_ms" => {
                Some(serde_json::Value::Number(self.global_timeout_ms.into()))
            }
            "observability.enable_tracing" | "enable_tracing" => {
                Some(serde_json::Value::Bool(self.enable_tracing))
            }
            "observability.enable_metrics" | "enable_metrics" => {
                Some(serde_json::Value::Bool(self.enable_metrics))
            }
            "modules_path" => self
                .modules_path
                .as_ref()
                .map(|p| serde_json::Value::String(p.to_string_lossy().into_owned())),
            _ => None,
        }
    }

    /// Try to set a typed field from a dot-path key. Returns true if matched.
    fn set_typed_field(&mut self, key: &str, value: &serde_json::Value) -> bool {
        match key {
            "executor.max_call_depth" | "max_call_depth" => {
                if let Some(n) = value.as_u64() {
                    self.max_call_depth = n as u32;
                    return true;
                }
            }
            "executor.max_module_repeat" | "max_module_repeat" => {
                if let Some(n) = value.as_u64() {
                    self.max_module_repeat = n as u32;
                    return true;
                }
            }
            "executor.default_timeout_ms" | "default_timeout_ms" => {
                if let Some(n) = value.as_u64() {
                    self.default_timeout_ms = n;
                    return true;
                }
            }
            "executor.global_timeout_ms" | "global_timeout_ms" => {
                if let Some(n) = value.as_u64() {
                    self.global_timeout_ms = n;
                    return true;
                }
            }
            "observability.enable_tracing" | "enable_tracing" => {
                if let Some(b) = value.as_bool() {
                    self.enable_tracing = b;
                    return true;
                }
            }
            "observability.enable_metrics" | "enable_metrics" => {
                if let Some(b) = value.as_bool() {
                    self.enable_metrics = b;
                    return true;
                }
            }
            "modules_path" => {
                if let Some(s) = value.as_str() {
                    self.modules_path = Some(PathBuf::from(s));
                    return true;
                }
            }
            _ => {}
        }
        false
    }

    /// Convert an env-var suffix to a dot-path config key.
    ///
    /// Convention (matches Python reference):
    ///   - Single `_` → `.` (section separator)
    ///   - Double `__` → literal `_` (underscore within a field name)
    ///
    /// Example: `EXECUTOR_MAX__CALL__DEPTH` → `executor.max_call_depth`
    ///
    /// So to set `max_call_depth` via env, use `APCORE_EXECUTOR_MAX__CALL__DEPTH`.
    fn env_key_to_dot_path(raw: &str) -> String {
        let lower = raw.to_lowercase();
        let chars: Vec<char> = lower.chars().collect();
        let mut result = String::with_capacity(chars.len());
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '_' {
                if i + 1 < chars.len() && chars[i + 1] == '_' {
                    result.push('_');
                    i += 2;
                } else {
                    result.push('.');
                    i += 1;
                }
            } else {
                result.push(chars[i]);
                i += 1;
            }
        }
        result
    }

    fn coerce_env_value(value: &str) -> serde_json::Value {
        if value.eq_ignore_ascii_case("true") {
            return serde_json::Value::Bool(true);
        }
        if value.eq_ignore_ascii_case("false") {
            return serde_json::Value::Bool(false);
        }
        if let Ok(n) = value.parse::<i64>() {
            return serde_json::Value::Number(n.into());
        }
        if let Ok(f) = value.parse::<f64>() {
            if let Some(n) = serde_json::Number::from_f64(f) {
                return serde_json::Value::Number(n);
            }
        }
        serde_json::Value::String(value.to_string())
    }
}

// ---------------------------------------------------------------------------
// Built-in namespace initialization (§9.15)
// ---------------------------------------------------------------------------

fn init_builtin_namespaces() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let namespaces = vec![
            NamespaceRegistration {
                name: "observability".to_string(),
                env_prefix: Some("APCORE__OBSERVABILITY".to_string()),
                defaults: Some(serde_json::json!({
                    "tracing": { "enabled": false, "sampling_rate": 1.0 },
                    "metrics": { "enabled": false }
                })),
                schema: None,
            },
            NamespaceRegistration {
                name: "sys_modules".to_string(),
                env_prefix: Some("APCORE__SYS".to_string()),
                defaults: Some(serde_json::json!({
                    "enabled": true,
                    "health": { "enabled": true },
                    "manifest": { "enabled": true },
                    "usage": { "enabled": true, "retention_hours": 168, "bucketing_strategy": "hourly" },
                    "control": { "enabled": true },
                    "events": {
                        "enabled": false,
                        "subscribers": [],
                        "thresholds": { "error_rate": 0.1, "latency_p99_ms": 5000.0 }
                    },
                    "error_history": {
                        "max_entries_per_module": 50,
                        "max_total_entries": 1000
                    }
                })),
                schema: None,
            },
        ];
        for ns in namespaces {
            // Ignore duplicate errors on re-init
            let _ = Config::register_namespace(ns);
        }
    });
}

// ---------------------------------------------------------------------------
// Config discovery (§9.14)
// ---------------------------------------------------------------------------

fn discover_config_file() -> Option<std::path::PathBuf> {
    if let Ok(env_path) = std::env::var("APCORE_CONFIG_FILE") {
        if !env_path.is_empty() {
            return Some(std::path::PathBuf::from(env_path));
        }
    }

    let cwd_candidates = ["project.yaml", "project.yml", "apcore.yaml", "apcore.yml"];
    for name in &cwd_candidates {
        let p = std::path::Path::new(name);
        if p.exists() {
            return Some(p.to_path_buf());
        }
    }

    if let Some(home) = dirs_home() {
        #[cfg(target_os = "macos")]
        let xdg = home
            .join("Library")
            .join("Application Support")
            .join("apcore")
            .join("config.yaml");
        #[cfg(not(target_os = "macos"))]
        let xdg = home.join(".config").join("apcore").join("config.yaml");

        if xdg.exists() {
            return Some(xdg);
        }

        let legacy = home.join(".apcore").join("config.yaml");
        if legacy.exists() {
            return Some(legacy);
        }
    }

    None
}

fn dirs_home() -> Option<std::path::PathBuf> {
    std::env::var("HOME").ok().map(std::path::PathBuf::from)
}

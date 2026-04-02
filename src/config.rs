// APCore Protocol — Configuration
// Spec reference: Configuration loading, validation, and environment variable overrides (Algorithm A12)

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

/// Default maximum nesting depth for env var key conversion.
pub const DEFAULT_MAX_DEPTH: usize = 5;

/// Environment variable key conversion strategy for a namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EnvStyle {
    /// Single `_` → `.` (section separator), double `__` → literal `_`.
    Nested,
    /// Suffix is lowercased as-is; no separator conversion.
    Flat,
    /// Match against defaults tree structure; fall back to Nested.
    #[default]
    Auto,
}

/// Registration info for a Config Bus namespace.
#[derive(Debug, Clone)]
pub struct NamespaceRegistration {
    pub name: String,
    /// Env var prefix. `None` = auto-derive from name (uppercase, `-` → `_`).
    pub env_prefix: Option<String>,
    pub defaults: Option<serde_json::Value>,
    pub schema: Option<serde_json::Value>,
    pub env_style: EnvStyle,
    pub max_depth: usize,
    /// Explicit bare env var → config key mapping (e.g. `"REDIS_URL" → "cache_url"`).
    pub env_map: Option<HashMap<String, String>>,
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
/// Global bare env var → top-level config key mapping.
static GLOBAL_ENV_MAP: OnceLock<RwLock<HashMap<String, String>>> = OnceLock::new();
/// Tracks all claimed env var names (for conflict detection).
static ENV_MAP_CLAIMED: OnceLock<RwLock<HashMap<String, String>>> = OnceLock::new();

fn global_ns_registry() -> &'static RwLock<HashMap<String, NamespaceRegistration>> {
    GLOBAL_NS_REGISTRY.get_or_init(|| RwLock::new(HashMap::new()))
}

fn global_env_map() -> &'static RwLock<HashMap<String, String>> {
    GLOBAL_ENV_MAP.get_or_init(|| RwLock::new(HashMap::new()))
}

fn env_map_claimed() -> &'static RwLock<HashMap<String, String>> {
    ENV_MAP_CLAIMED.get_or_init(|| RwLock::new(HashMap::new()))
}

const RESERVED_NAMESPACES: &[&str] = &["apcore", "_config"];

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
    #[serde(flatten)]
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
        config.detect_mode();
        init_builtin_namespaces();
        config.apply_env_overrides();
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
        config.detect_mode();
        init_builtin_namespaces();
        config.apply_env_overrides();
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
        config.detect_mode();
        init_builtin_namespaces();
        config.apply_env_overrides();
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

    pub fn register_namespace(mut reg: NamespaceRegistration) -> Result<(), ModuleError> {
        if RESERVED_NAMESPACES.contains(&reg.name.as_str()) {
            return Err(ModuleError::config_namespace_reserved(&reg.name));
        }
        // Auto-derive env_prefix from name if not provided.
        if reg.env_prefix.is_none() {
            reg.env_prefix = Some(reg.name.to_uppercase().replace('-', "_"));
        }
        let mut map = global_ns_registry()
            .write()
            .map_err(|_| ModuleError::config_mount_error(&reg.name, "registry lock poisoned"))?;
        if map.contains_key(&reg.name) {
            return Err(ModuleError::config_namespace_duplicate(&reg.name));
        }
        // Check for duplicate env_prefix.
        let prefix = reg.env_prefix.as_deref().unwrap_or("");
        for existing in map.values() {
            if existing.env_prefix.as_deref() == Some(prefix) {
                return Err(ModuleError::config_env_prefix_conflict(prefix));
            }
        }
        // Validate env_map: no env var can be claimed twice.
        if let Some(ref em) = reg.env_map {
            let claimed = env_map_claimed().read().unwrap_or_else(|e| e.into_inner());
            for env_var in em.keys() {
                if let Some(owner) = claimed.get(env_var) {
                    return Err(ModuleError::config_env_map_conflict(env_var, owner));
                }
            }
            drop(claimed);
            let mut claimed = env_map_claimed().write().unwrap_or_else(|e| e.into_inner());
            for env_var in em.keys() {
                claimed.insert(env_var.clone(), reg.name.clone());
            }
        }
        map.insert(reg.name.clone(), reg);
        Ok(())
    }

    /// Register global bare env var → top-level config key mappings.
    pub fn env_map(mapping: HashMap<String, String>) -> Result<(), ModuleError> {
        let claimed_lock = env_map_claimed();
        let claimed = claimed_lock.read().unwrap_or_else(|e| e.into_inner());
        for env_var in mapping.keys() {
            if let Some(owner) = claimed.get(env_var) {
                return Err(ModuleError::config_env_map_conflict(env_var, owner));
            }
        }
        drop(claimed);
        let mut claimed = claimed_lock.write().unwrap_or_else(|e| e.into_inner());
        let mut gmap = global_env_map().write().unwrap_or_else(|e| e.into_inner());
        for (env_var, config_key) in mapping {
            claimed.insert(env_var.clone(), "__global__".to_string());
            gmap.insert(env_var, config_key);
        }
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
        let gmap = global_env_map().read().unwrap_or_else(|e| e.into_inner());

        // Build namespace env_map lookup.
        let mut ns_env_maps: HashMap<&str, (&str, &str)> = HashMap::new();
        for reg in registry.values() {
            if let Some(ref em) = reg.env_map {
                for (env_var, config_key) in em {
                    ns_env_maps.insert(env_var.as_str(), (reg.name.as_str(), config_key.as_str()));
                }
            }
        }

        // Prefix table: sorted by length descending for longest-prefix-match.
        let mut prefixed: Vec<&NamespaceRegistration> = registry
            .values()
            .filter(|r| r.env_prefix.is_some())
            .collect();
        prefixed.sort_by(|a, b| {
            b.env_prefix
                .as_ref()
                .map_or(0, |p| p.len())
                .cmp(&a.env_prefix.as_ref().map_or(0, |p| p.len()))
        });

        for (env_key, env_value) in std::env::vars() {
            let parsed = Self::coerce_env_value(&env_value);

            // 1. Global env_map (bare env var → top-level key).
            if let Some(config_key) = gmap.get(&env_key) {
                self.set(config_key, parsed);
                continue;
            }

            // 2. Namespace env_map (bare env var → namespace key).
            if let Some(&(ns_name, config_key)) = ns_env_maps.get(env_key.as_str()) {
                let full_path = format!("{ns_name}.{config_key}");
                self.set(&full_path, parsed);
                continue;
            }

            // 3. Prefix-based dispatch.
            let mut matched = false;
            for reg in &prefixed {
                let prefix = reg.env_prefix.as_deref().unwrap_or("");
                if let Some(suffix) = env_key.strip_prefix(prefix) {
                    let suffix = suffix.strip_prefix('_').unwrap_or(suffix);
                    if suffix.is_empty() {
                        continue;
                    }
                    let key = Self::resolve_env_suffix(suffix, reg);
                    let full_path = format!("{}.{key}", reg.name);
                    tracing::debug!(env = %env_key, path = %full_path, "Applying namespace env override");
                    self.set(&full_path, parsed.clone());
                    matched = true;
                    break;
                }
            }
            // Fallback: legacy APCORE_ prefix → route into the "apcore" sub-namespace.
            if !matched {
                if let Some(suffix) = env_key.strip_prefix("APCORE_") {
                    let dot_path = Self::env_key_to_dot_path(suffix);
                    let full_path = format!("apcore.{dot_path}");
                    tracing::debug!(env = %env_key, path = %full_path, "Applying legacy env override in namespace mode");
                    self.set(&full_path, parsed);
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
        Self::env_key_to_dot_path_with_depth(raw, usize::MAX)
    }

    /// Convert env var suffix to dot-path, stopping at `max_depth` segments.
    fn env_key_to_dot_path_with_depth(raw: &str, max_depth: usize) -> String {
        let lower = raw.to_lowercase();
        let chars: Vec<char> = lower.chars().collect();
        let mut result = String::with_capacity(chars.len());
        let mut dot_count: usize = 0;
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '_' {
                if i + 1 < chars.len() && chars[i + 1] == '_' {
                    result.push('_'); // double __ → literal _
                    i += 2;
                } else if dot_count < max_depth.saturating_sub(1) {
                    result.push('.');
                    dot_count += 1;
                    i += 1;
                } else {
                    result.push('_'); // depth limit reached
                    i += 1;
                }
            } else {
                result.push(chars[i]);
                i += 1;
            }
        }
        result
    }

    /// Try to match suffix against keys in a JSON object tree (recursive).
    fn match_suffix_to_tree(
        suffix: &str,
        tree: &serde_json::Map<String, serde_json::Value>,
        depth: usize,
        max_depth: usize,
    ) -> Option<String> {
        // 1. Try full suffix as a flat key.
        if tree.contains_key(suffix) {
            return Some(suffix.to_string());
        }
        // 2. Depth limit.
        if depth >= max_depth.saturating_sub(1) {
            return None;
        }
        // 3. Try splitting at each underscore.
        for (i, ch) in suffix.char_indices() {
            if ch != '_' || i == 0 || i == suffix.len() - 1 {
                continue;
            }
            let prefix_part = &suffix[..i];
            let remainder = &suffix[i + 1..];
            if let Some(serde_json::Value::Object(subtree)) = tree.get(prefix_part) {
                if let Some(sub) =
                    Self::match_suffix_to_tree(remainder, subtree, depth + 1, max_depth)
                {
                    return Some(format!("{prefix_part}.{sub}"));
                }
            }
        }
        None
    }

    /// Resolve an env var suffix based on the registration's env_style.
    fn resolve_env_suffix(suffix: &str, reg: &NamespaceRegistration) -> String {
        match reg.env_style {
            EnvStyle::Flat => suffix.to_lowercase(),
            EnvStyle::Auto => {
                let lower = suffix.to_lowercase();
                if let Some(serde_json::Value::Object(tree)) = reg.defaults.as_ref() {
                    if let Some(resolved) =
                        Self::match_suffix_to_tree(&lower, tree, 0, reg.max_depth)
                    {
                        return resolved;
                    }
                }
                // Fall back to nested with depth.
                Self::env_key_to_dot_path_with_depth(suffix, reg.max_depth)
            }
            EnvStyle::Nested => Self::env_key_to_dot_path_with_depth(suffix, reg.max_depth),
        }
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
                env_prefix: Some("APCORE_OBSERVABILITY".to_string()),
                defaults: Some(serde_json::json!({
                    "tracing": { "enabled": false, "sampling_rate": 1.0 },
                    "metrics": { "enabled": false }
                })),
                schema: None,
                env_style: EnvStyle::Nested,
                max_depth: DEFAULT_MAX_DEPTH,
                env_map: None,
            },
            NamespaceRegistration {
                name: "sys_modules".to_string(),
                env_prefix: Some("APCORE_SYS".to_string()),
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
                env_style: EnvStyle::Nested,
                max_depth: DEFAULT_MAX_DEPTH,
                env_map: None,
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

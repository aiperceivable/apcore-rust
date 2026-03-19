// APCore Protocol — Configuration
// Spec reference: Configuration loading, validation, and environment variable overrides (Algorithm A12)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::errors::{ErrorCode, ModuleError};

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
        config
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

    // --- Private helpers ---

    /// Apply APCORE_* environment variable overrides to both typed fields and settings.
    fn apply_env_overrides(&mut self) {
        for (key, value) in std::env::vars() {
            if let Some(suffix) = key.strip_prefix("APCORE_") {
                let dot_path = Self::env_key_to_dot_path(suffix);
                let parsed = Self::coerce_env_value(&value);
                tracing::debug!(
                    "Applying env override: {} = {:?} (from {})",
                    dot_path,
                    parsed,
                    key
                );
                self.set(&dot_path, parsed);
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

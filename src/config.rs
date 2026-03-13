use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::errors::ModuleError;

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
        }
    }
}

impl Config {
    pub fn from_json_file(path: &std::path::Path) -> Result<Self, ModuleError> {
        todo!()
    }

    pub fn from_yaml_file(path: &std::path::Path) -> Result<Self, ModuleError> {
        todo!()
    }

    pub fn validate(&self) -> Result<(), ModuleError> {
        todo!()
    }

    pub fn from_defaults() -> Self {
        let mut config = Self::default();
        for (key, value) in std::env::vars() {
            if let Some(suffix) = key.strip_prefix("APCORE_") {
                let dot_path = Self::env_key_to_dot_path(suffix);
                let parsed = Self::coerce_env_value(&value);
                config.set(&dot_path, parsed);
            }
        }
        config
    }

    pub fn get(&self, key: &str) -> Option<serde_json::Value> {
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

    pub fn set(&mut self, key: &str, value: serde_json::Value) {
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

    pub fn reload(&mut self) -> Result<(), ModuleError> {
        todo!("Config.reload() — re-read config from file")
    }

    pub fn load(path: &std::path::Path) -> Result<Self, ModuleError> {
        todo!("Config.load() — auto-detect format by extension")
    }

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

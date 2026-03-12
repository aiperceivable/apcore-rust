// APCore Protocol — Configuration
// Spec reference: Config loading and validation

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::errors::ModuleError;

/// APCore configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub modules_path: Option<PathBuf>,
    #[serde(default)]
    pub max_call_depth: u32,
    #[serde(default)]
    pub max_call_frequency: u32,
    #[serde(default)]
    pub default_timeout_ms: u64,
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
            max_call_depth: 10,
            max_call_frequency: 100,
            default_timeout_ms: 30000,
            enable_tracing: false,
            enable_metrics: false,
            settings: HashMap::new(),
        }
    }
}

impl Config {
    /// Load configuration from a JSON file.
    pub fn from_json_file(path: &std::path::Path) -> Result<Self, ModuleError> {
        // TODO: Implement
        todo!()
    }

    /// Load configuration from a YAML file.
    pub fn from_yaml_file(path: &std::path::Path) -> Result<Self, ModuleError> {
        // TODO: Implement
        todo!()
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), ModuleError> {
        // TODO: Implement
        todo!()
    }
}

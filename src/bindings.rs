// APCore Protocol — Binding loader
// Spec reference: Module binding resolution from external sources

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

use crate::errors::ModuleError;

/// Describes a binding target.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BindingTarget {
    pub module_name: String,
    pub callable: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema_path: Option<String>,
}

/// A resolved binding definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BindingDefinition {
    pub name: String,
    pub target: BindingTarget,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Loads and resolves module bindings from files or configuration.
#[derive(Debug)]
pub struct BindingLoader {
    bindings: HashMap<String, BindingDefinition>,
}

impl BindingLoader {
    /// Create a new empty binding loader.
    pub fn new() -> Self {
        Self {
            bindings: HashMap::new(),
        }
    }

    /// Load bindings from a JSON file.
    pub fn load_from_file(&mut self, path: &Path) -> Result<(), ModuleError> {
        // TODO: Implement
        todo!()
    }

    /// Load bindings from a YAML file.
    pub fn load_from_yaml(&mut self, path: &Path) -> Result<(), ModuleError> {
        // TODO: Implement
        todo!()
    }

    /// Resolve a binding by name.
    pub fn resolve(&self, name: &str) -> Result<&BindingDefinition, ModuleError> {
        // TODO: Implement
        todo!()
    }

    /// List all loaded binding names.
    pub fn list_bindings(&self) -> Vec<&str> {
        self.bindings.keys().map(|k| k.as_str()).collect()
    }
}

impl Default for BindingLoader {
    fn default() -> Self {
        Self::new()
    }
}

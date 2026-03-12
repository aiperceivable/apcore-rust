// APCore Protocol — Schema loader
// Spec reference: Loading schemas from files and inline definitions

use std::collections::HashMap;
use std::path::Path;

use crate::errors::ModuleError;

/// Strategy for loading schemas when both YAML and native definitions exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaStrategy {
    /// Prefer YAML schema files over native code definitions.
    YamlFirst,
    /// Prefer native code definitions over YAML files.
    NativeFirst,
    /// Use only YAML files; ignore native definitions.
    YamlOnly,
}

/// Loads JSON schemas from various sources.
#[derive(Debug)]
pub struct SchemaLoader {
    schemas: HashMap<String, serde_json::Value>,
    pub strategy: SchemaStrategy,
}

impl SchemaLoader {
    /// Create a new schema loader with the default strategy.
    pub fn new() -> Self {
        Self {
            schemas: HashMap::new(),
            strategy: SchemaStrategy::YamlFirst,
        }
    }

    /// Create a schema loader with a specific strategy.
    pub fn with_strategy(strategy: SchemaStrategy) -> Self {
        Self {
            schemas: HashMap::new(),
            strategy,
        }
    }

    /// Load a schema from a JSON/YAML file.
    pub fn load_from_file(&mut self, name: &str, path: &Path) -> Result<(), ModuleError> {
        // TODO: Implement
        todo!()
    }

    /// Load a schema from a JSON value.
    pub fn load_from_value(&mut self, name: &str, schema: serde_json::Value) -> Result<(), ModuleError> {
        // TODO: Implement
        todo!()
    }

    /// Get a loaded schema by name.
    pub fn get(&self, name: &str) -> Option<&serde_json::Value> {
        self.schemas.get(name)
    }

    /// List all loaded schema names.
    pub fn list(&self) -> Vec<&str> {
        self.schemas.keys().map(|k| k.as_str()).collect()
    }
}

impl Default for SchemaLoader {
    fn default() -> Self {
        Self::new()
    }
}

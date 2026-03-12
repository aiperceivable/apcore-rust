// APCore Protocol — Schema loader
// Spec reference: Loading schemas from files and inline definitions

use std::collections::HashMap;
use std::path::Path;

use crate::errors::ModuleError;

/// Loads JSON schemas from various sources.
#[derive(Debug)]
pub struct SchemaLoader {
    schemas: HashMap<String, serde_json::Value>,
}

impl SchemaLoader {
    /// Create a new schema loader.
    pub fn new() -> Self {
        Self {
            schemas: HashMap::new(),
        }
    }

    /// Load a schema from a JSON file.
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

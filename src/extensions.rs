// APCore Protocol — Extension points
// Spec reference: Extension mechanism for pluggable behavior

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::errors::ModuleError;

/// Defines a named extension point where plugins can hook in.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionPoint {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub required: bool,
}

/// Manages registration and invocation of extensions.
#[derive(Debug)]
pub struct ExtensionManager {
    extension_points: HashMap<String, ExtensionPoint>,
    extensions: HashMap<String, Vec<Box<dyn Extension>>>,
}

/// Trait implemented by extension plugins.
#[async_trait]
pub trait Extension: Send + Sync + std::fmt::Debug {
    /// The name of the extension point this extension attaches to.
    fn extension_point(&self) -> &str;

    /// Execute the extension logic.
    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value, ModuleError>;
}

impl ExtensionManager {
    /// Create a new extension manager.
    pub fn new() -> Self {
        Self {
            extension_points: HashMap::new(),
            extensions: HashMap::new(),
        }
    }

    /// Register an extension point.
    pub fn register_point(&mut self, point: ExtensionPoint) -> Result<(), ModuleError> {
        self.extension_points.insert(point.name.clone(), point);
        Ok(())
    }

    /// Register an extension for a given extension point.
    pub fn register_extension(&mut self, extension: Box<dyn Extension>) -> Result<(), ModuleError> {
        let point_name = extension.extension_point().to_string();
        self.extensions
            .entry(point_name)
            .or_default()
            .push(extension);
        Ok(())
    }

    /// Invoke all extensions registered at the given extension point.
    pub async fn invoke(
        &self,
        point_name: &str,
        input: serde_json::Value,
    ) -> Result<Vec<serde_json::Value>, ModuleError> {
        let mut results = Vec::new();

        if let Some(exts) = self.extensions.get(point_name) {
            for ext in exts {
                let result = ext.execute(input.clone()).await?;
                results.push(result);
            }
        }

        Ok(results)
    }

    /// List all registered extension point names.
    pub fn list_points(&self) -> Vec<&str> {
        self.extension_points.keys().map(|k| k.as_str()).collect()
    }
}

impl Default for ExtensionManager {
    fn default() -> Self {
        Self::new()
    }
}

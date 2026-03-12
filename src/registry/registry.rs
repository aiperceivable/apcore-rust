// APCore Protocol — Registry, Discoverer, ModuleValidator
// Spec reference: Module registration, discovery, validation, and descriptors

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::errors::ModuleError;
use crate::module::{Module, ModuleAnnotations, ValidationResult};

/// Metadata descriptor for a registered module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleDescriptor {
    pub name: String,
    pub annotations: ModuleAnnotations,
    pub input_schema: serde_json::Value,
    pub output_schema: serde_json::Value,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub dependencies: Vec<DependencyInfo>,
}

/// Dependency information for a module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyInfo {
    pub name: String,
    pub version_constraint: String,
    #[serde(default)]
    pub optional: bool,
}

/// A module found via discovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredModule {
    pub name: String,
    pub source: String,
    pub descriptor: ModuleDescriptor,
}

/// Trait for discovering modules from external sources.
#[async_trait]
pub trait Discoverer: Send + Sync {
    /// Discover available modules.
    async fn discover(&self) -> Result<Vec<DiscoveredModule>, ModuleError>;
}

/// Trait for validating module implementations.
pub trait ModuleValidator: Send + Sync {
    /// Validate a module against the protocol contract.
    fn validate(&self, module: &dyn Module, descriptor: &ModuleDescriptor) -> ValidationResult;
}

/// Central registry of modules.
pub struct Registry {
    modules: HashMap<String, Box<dyn Module>>,
    descriptors: HashMap<String, ModuleDescriptor>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry")
            .field("modules", &self.modules.keys().collect::<Vec<_>>())
            .field("descriptors", &self.descriptors)
            .finish()
    }
}

impl Registry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            modules: HashMap::new(),
            descriptors: HashMap::new(),
        }
    }

    /// Register a module with the given name.
    pub fn register(
        &mut self,
        name: &str,
        module: Box<dyn Module>,
        descriptor: ModuleDescriptor,
    ) -> Result<(), ModuleError> {
        // TODO: Implement
        todo!()
    }

    /// Unregister a module by name.
    pub fn unregister(&mut self, name: &str) -> Result<(), ModuleError> {
        // TODO: Implement
        todo!()
    }

    /// Get a reference to a module by name.
    pub fn get(&self, name: &str) -> Option<&dyn Module> {
        self.modules.get(name).map(|m| m.as_ref())
    }

    /// Get the descriptor for a module by name.
    pub fn get_descriptor(&self, name: &str) -> Option<&ModuleDescriptor> {
        self.descriptors.get(name)
    }

    /// List all registered module names.
    pub fn list(&self) -> Vec<&str> {
        self.modules.keys().map(|k| k.as_str()).collect()
    }

    /// Check if a module is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.modules.contains_key(name)
    }

    /// Discover and register modules from a discoverer.
    pub async fn discover_and_register(
        &mut self,
        _discoverer: &dyn Discoverer,
    ) -> Result<Vec<String>, ModuleError> {
        // TODO: Implement
        todo!()
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

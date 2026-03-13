// APCore Protocol — Registry, Discoverer, ModuleValidator
// Spec reference: Module registration, discovery, validation, and descriptors

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

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
    pub tags: Vec<String>,
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
///
/// TODO(L-012): Add VersionedStore support for multi-version module management.
/// This requires a separate `versioned_store` module with version-aware storage,
/// conflict resolution, and migration support. For now, modules are single-version.
pub struct Registry {
    modules: HashMap<String, Box<dyn Module>>,
    descriptors: HashMap<String, ModuleDescriptor>,
    /// Reference counts for safe hot-reload — prevents unloading while in use.
    ref_counts: HashMap<String, usize>,
    /// Modules marked for unload (draining active requests before removal).
    draining: HashSet<String>,
    /// Drain completion notification — signaled when a draining module reaches zero refs.
    drain_events: HashMap<String, tokio::sync::Notify>,
    /// Event callbacks keyed by event name (e.g. "register", "unregister").
    callbacks: HashMap<String, Vec<Box<dyn Fn(&str, &dyn Module) + Send + Sync>>>,
    /// Case-insensitive lookup: lowercase name -> canonical name.
    lowercase_map: HashMap<String, String>,
    /// Cached JSON schemas for registered modules.
    schema_cache: HashMap<String, serde_json::Value>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry")
            .field("modules", &self.modules.keys().collect::<Vec<_>>())
            .field("descriptors", &self.descriptors)
            .field("ref_counts", &self.ref_counts)
            .field("draining", &self.draining)
            .field("drain_events_keys", &self.drain_events.keys().collect::<Vec<_>>())
            .field("callbacks_keys", &self.callbacks.keys().collect::<Vec<_>>())
            .field("lowercase_map", &self.lowercase_map)
            .field("schema_cache_keys", &self.schema_cache.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Registry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            modules: HashMap::new(),
            descriptors: HashMap::new(),
            ref_counts: HashMap::new(),
            draining: HashSet::new(),
            drain_events: HashMap::new(),
            callbacks: HashMap::new(),
            lowercase_map: HashMap::new(),
            schema_cache: HashMap::new(),
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

    /// Get the definition (descriptor) for a module by name.
    pub fn get_definition(&self, name: &str) -> Option<&ModuleDescriptor> {
        self.descriptors.get(name)
    }

    /// List registered module names with optional filtering.
    ///
    /// - `tags`: if provided, only return modules whose descriptor annotations
    ///   contain ALL of the specified tags.
    /// - `prefix`: if provided, only return modules whose name starts with the prefix.
    /// - When both are `None`, returns all registered module names.
    pub fn list(&self, tags: Option<&[&str]>, prefix: Option<&str>) -> Vec<&str> {
        self.modules
            .keys()
            .filter(|name| {
                if let Some(pfx) = prefix {
                    if !name.starts_with(pfx) {
                        return false;
                    }
                }
                if let Some(required_tags) = tags {
                    if let Some(desc) = self.descriptors.get(name.as_str()) {
                        let module_tags = &desc.tags;
                        if !required_tags.iter().all(|t| module_tags.contains(&t.to_string())) {
                            return false;
                        }
                    } else {
                        // No descriptor means no tags — exclude when tag filter is active
                        return false;
                    }
                }
                true
            })
            .map(|k| k.as_str())
            .collect()
    }

    /// Check if a module is registered.
    pub fn has(&self, name: &str) -> bool {
        self.modules.contains_key(name)
    }

    /// Discover and register modules from a discoverer.
    pub async fn discover(
        &mut self,
        _discoverer: &dyn Discoverer,
    ) -> Result<Vec<String>, ModuleError> {
        // TODO: Implement
        todo!()
    }

    /// Register a module without validation.
    pub fn register_internal(&mut self, name: &str, module: Box<dyn Module>, descriptor: ModuleDescriptor) -> Result<(), ModuleError> {
        todo!("Registry.register_internal() — register without validation")
    }

    /// Iterate over modules.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &dyn Module)> {
        todo!("Registry.iter() — iterate over modules");
        #[allow(unreachable_code)]
        std::iter::empty::<(&str, &dyn Module)>()
    }

    /// Human-readable module description.
    pub fn describe(&self, name: &str) -> String {
        todo!("Registry.describe() — human-readable module description")
    }

    /// Draining-aware unregister.
    pub async fn safe_unregister(&mut self, name: &str, timeout_ms: u64) -> Result<bool, ModuleError> {
        todo!("Registry.safe_unregister() — draining-aware unregister")
    }

    /// Ref-counted module access.
    pub fn acquire(&self, name: &str) -> Result<&dyn Module, ModuleError> {
        todo!("Registry.acquire() — ref-counted module access")
    }

    /// Check if a module is draining.
    pub fn is_draining(&self, name: &str) -> bool {
        self.draining.contains(name)
    }

    /// Event subscription.
    pub fn on(&mut self, event: &str, callback: Box<dyn Fn(&str, &dyn Module) + Send + Sync>) {
        todo!("Registry.on() — event subscription")
    }

    /// Filesystem watching.
    pub async fn watch(&mut self) -> Result<(), ModuleError> {
        todo!("Registry.watch() — filesystem watching")
    }

    /// Stop filesystem watching.
    pub fn unwatch(&mut self) {
        todo!("Registry.unwatch()")
    }

    /// Set the discoverer.
    pub fn set_discoverer(&mut self, discoverer: Box<dyn Discoverer>) {
        todo!("Registry.set_discoverer()")
    }

    /// Set the validator.
    pub fn set_validator(&mut self, validator: Box<dyn ModuleValidator>) {
        todo!("Registry.set_validator()")
    }

    /// Return count of registered modules.
    pub fn count(&self) -> usize {
        self.modules.len()
    }

    /// Return all module IDs, sorted alphabetically.
    pub fn module_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.modules.keys().cloned().collect();
        ids.sort();
        ids
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

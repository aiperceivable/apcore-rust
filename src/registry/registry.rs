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

/// Type alias for the event callback closure.
type ModuleCallbackFn = dyn Fn(&str, &dyn Module) + Send + Sync;

/// Reserved words that cannot be used as the first segment of a module ID.
///
/// Aligned with the Python and TypeScript SDKs to ensure cross-language consistency.
pub const RESERVED_WORDS: &[&str] = &["system", "internal", "core", "apcore", "plugin", "schema", "acl"];

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
    drain_events: HashMap<String, std::sync::Arc<tokio::sync::Notify>>,
    /// Event callbacks keyed by event name (e.g. "register", "unregister").
    callbacks: HashMap<String, Vec<Box<ModuleCallbackFn>>>,
    /// Case-insensitive lookup: lowercase name -> canonical name.
    lowercase_map: HashMap<String, String>,
    /// Cached JSON schemas for registered modules.
    schema_cache: HashMap<String, serde_json::Value>,
    /// Optional discoverer for module discovery.
    discoverer: Option<Box<dyn Discoverer>>,
    /// Optional validator for module validation.
    validator: Option<Box<dyn ModuleValidator>>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry")
            .field("modules", &self.modules.keys().collect::<Vec<_>>())
            .field("descriptors", &self.descriptors)
            .field("ref_counts", &self.ref_counts)
            .field("draining", &self.draining)
            .field(
                "drain_events_keys",
                &self.drain_events.keys().collect::<Vec<_>>(),
            )
            .field("callbacks_keys", &self.callbacks.keys().collect::<Vec<_>>())
            .field("lowercase_map", &self.lowercase_map)
            .field(
                "schema_cache_keys",
                &self.schema_cache.keys().collect::<Vec<_>>(),
            )
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
            discoverer: None,
            validator: None,
        }
    }

    /// Register a module with the given name.
    pub fn register(
        &mut self,
        name: &str,
        module: Box<dyn Module>,
        descriptor: ModuleDescriptor,
    ) -> Result<(), ModuleError> {
        // Reject module IDs whose first segment is a reserved word.
        let first_segment = name.split('.').next().unwrap_or(name);
        if RESERVED_WORDS.contains(&first_segment) {
            return Err(ModuleError::new(
                crate::errors::ErrorCode::GeneralInvalidInput,
                format!(
                    "Module ID '{}' uses reserved word '{}' as its first segment",
                    name, first_segment
                ),
            ));
        }

        if self.modules.contains_key(name) {
            return Err(ModuleError::new(
                crate::errors::ErrorCode::ModuleLoadError,
                format!("Module '{}' is already registered", name),
            ));
        }

        // Run validation callbacks if a validator is set
        if let Some(ref validator) = self.validator {
            let result = validator.validate(module.as_ref(), &descriptor);
            if !result.valid {
                return Err(ModuleError::new(
                    crate::errors::ErrorCode::ModuleLoadError,
                    format!(
                        "Module '{}' failed validation: {}",
                        name,
                        result.errors.join(", ")
                    ),
                ));
            }
        }

        // Cache the schema
        let schema = serde_json::json!({
            "input": descriptor.input_schema,
            "output": descriptor.output_schema,
        });
        self.schema_cache.insert(name.to_string(), schema);

        // Update lowercase map
        self.lowercase_map
            .insert(name.to_lowercase(), name.to_string());

        // Call on_load lifecycle hook
        module.on_load();

        // Store module and descriptor
        self.modules.insert(name.to_string(), module);
        self.descriptors.insert(name.to_string(), descriptor);

        // Fire "register" callbacks
        if let Some(cbs) = self.callbacks.get("register") {
            let m = self.modules.get(name).unwrap();
            for cb in cbs {
                cb(name, m.as_ref());
            }
        }

        Ok(())
    }

    /// Register a module with auto-generated descriptor.
    pub fn register_module(
        &mut self,
        name: &str,
        module: Box<dyn Module>,
    ) -> Result<(), ModuleError> {
        let descriptor = ModuleDescriptor {
            name: name.to_string(),
            annotations: ModuleAnnotations::default(),
            input_schema: module.input_schema(),
            output_schema: module.output_schema(),
            enabled: true,
            tags: vec![],
            dependencies: vec![],
        };
        self.register(name, module, descriptor)
    }

    /// Unregister a module by name.
    pub fn unregister(&mut self, name: &str) -> Result<(), ModuleError> {
        if !self.modules.contains_key(name) {
            return Err(ModuleError::new(
                crate::errors::ErrorCode::ModuleNotFound,
                format!("Module '{}' not found", name),
            ));
        }

        // Fire "unregister" callbacks before removal
        if let Some(cbs) = self.callbacks.get("unregister") {
            let m = self.modules.get(name).unwrap();
            for cb in cbs {
                cb(name, m.as_ref());
            }
        }

        // Call on_unload lifecycle hook
        if let Some(module) = self.modules.get(name) {
            module.on_unload();
        }

        // Remove from all maps
        self.modules.remove(name);
        self.descriptors.remove(name);
        self.lowercase_map.remove(&name.to_lowercase());
        self.schema_cache.remove(name);
        self.ref_counts.remove(name);
        self.draining.remove(name);
        self.drain_events.remove(name);

        Ok(())
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
                        if !required_tags
                            .iter()
                            .all(|t| module_tags.contains(&t.to_string()))
                        {
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
        discoverer: &dyn Discoverer,
    ) -> Result<Vec<String>, ModuleError> {
        let discovered = discoverer.discover().await?;
        let mut registered_names = Vec::new();

        for dm in discovered {
            // Discovery returns descriptors but not actual module implementations.
            // We record the name; actual module objects would need to be constructed
            // by a loader. For now, store the descriptor and track the name.
            self.descriptors.insert(dm.name.clone(), dm.descriptor);
            self.lowercase_map
                .insert(dm.name.to_lowercase(), dm.name.clone());
            registered_names.push(dm.name);
        }

        Ok(registered_names)
    }

    /// Register a module without validation.
    pub fn register_internal(
        &mut self,
        name: &str,
        module: Box<dyn Module>,
        descriptor: ModuleDescriptor,
    ) -> Result<(), ModuleError> {
        if self.modules.contains_key(name) {
            return Err(ModuleError::new(
                crate::errors::ErrorCode::ModuleLoadError,
                format!("Module '{}' is already registered", name),
            ));
        }

        // Cache the schema
        let schema = serde_json::json!({
            "input": descriptor.input_schema,
            "output": descriptor.output_schema,
        });
        self.schema_cache.insert(name.to_string(), schema);

        // Update lowercase map
        self.lowercase_map
            .insert(name.to_lowercase(), name.to_string());

        // Call on_load lifecycle hook
        module.on_load();

        // Store module and descriptor
        self.modules.insert(name.to_string(), module);
        self.descriptors.insert(name.to_string(), descriptor);

        // Fire "register" callbacks
        if let Some(cbs) = self.callbacks.get("register") {
            let m = self.modules.get(name).unwrap();
            for cb in cbs {
                cb(name, m.as_ref());
            }
        }

        Ok(())
    }

    /// Iterate over modules.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &dyn Module)> {
        self.modules
            .iter()
            .map(|(name, module)| (name.as_str(), module.as_ref()))
    }

    /// Human-readable module description.
    pub fn describe(&self, name: &str) -> String {
        match self.modules.get(name) {
            Some(module) => module.description().to_string(),
            None => "Module not found".to_string(),
        }
    }

    /// Draining-aware unregister.
    pub async fn safe_unregister(
        &mut self,
        name: &str,
        timeout_ms: u64,
    ) -> Result<bool, ModuleError> {
        if !self.modules.contains_key(name) {
            return Err(ModuleError::new(
                crate::errors::ErrorCode::ModuleNotFound,
                format!("Module '{}' not found", name),
            ));
        }

        // Mark as draining
        self.draining.insert(name.to_string());

        // Check if ref_count is already zero
        let current_refs = self.ref_counts.get(name).copied().unwrap_or(0);
        if current_refs == 0 {
            self.unregister(name)?;
            return Ok(true);
        }

        // Set up a notification for when draining completes
        let notify = std::sync::Arc::new(tokio::sync::Notify::new());
        self.drain_events.insert(name.to_string(), notify.clone());

        // Wait for ref_count to reach 0 or timeout
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(timeout_ms),
            notify.notified(),
        )
        .await;

        match result {
            Ok(_) => {
                // Drain completed, unregister
                self.unregister(name)?;
                Ok(true)
            }
            Err(_) => {
                // Timeout — remove draining flag but don't unregister
                self.draining.remove(name);
                self.drain_events.remove(name);
                Ok(false)
            }
        }
    }

    /// Ref-counted module access with explicit reference tracking.
    ///
    /// Increments the reference count for the module before returning it.
    /// Call `release_ref()` when done to decrement the count. This is required
    /// for `safe_unregister()` drain logic to work correctly.
    pub fn acquire_ref(&mut self, name: &str) -> Result<&dyn Module, ModuleError> {
        if self.draining.contains(name) {
            return Err(ModuleError::new(
                crate::errors::ErrorCode::ModuleNotFound,
                format!("Module '{}' is draining", name),
            ));
        }
        let module = self.modules.get(name).map(|m| m.as_ref()).ok_or_else(|| {
            ModuleError::new(
                crate::errors::ErrorCode::ModuleNotFound,
                format!("Module '{}' not found", name),
            )
        })?;
        *self.ref_counts.entry(name.to_string()).or_insert(0) += 1;
        Ok(module)
    }

    /// Decrement the reference count for a module. Notifies drain waiters when count reaches 0.
    pub fn release_ref(&mut self, name: &str) {
        if let Some(count) = self.ref_counts.get_mut(name) {
            if *count > 0 {
                *count -= 1;
            }
            if *count == 0 {
                self.ref_counts.remove(name);
                // Notify drain waiters
                if let Some(notify) = self.drain_events.get(name) {
                    notify.notify_one();
                }
            }
        }
    }

    /// Simple module access (no ref-counting).
    ///
    /// Checks draining status before returning the module reference.
    /// Note: For safe-unregister scenarios, use `acquire_ref()`/`release_ref()`
    /// instead, which track reference counts for drain logic.
    pub fn acquire(&self, name: &str) -> Result<&dyn Module, ModuleError> {
        if self.draining.contains(name) {
            return Err(ModuleError::new(
                crate::errors::ErrorCode::ModuleNotFound,
                format!("Module '{}' is draining", name),
            ));
        }
        self.modules.get(name).map(|m| m.as_ref()).ok_or_else(|| {
            ModuleError::new(
                crate::errors::ErrorCode::ModuleNotFound,
                format!("Module '{}' not found", name),
            )
        })
    }

    /// Check if a module is draining.
    pub fn is_draining(&self, name: &str) -> bool {
        self.draining.contains(name)
    }

    /// Event subscription.
    pub fn on(&mut self, event: &str, callback: Box<ModuleCallbackFn>) {
        self.callbacks
            .entry(event.to_string())
            .or_default()
            .push(callback);
    }

    /// Filesystem watching (no-op — filesystem watching is platform-specific).
    pub async fn watch(&mut self) -> Result<(), ModuleError> {
        // No-op: filesystem watching requires platform-specific dependencies
        // (e.g. notify crate). Stubbed for API compatibility.
        Ok(())
    }

    /// Stop filesystem watching (no-op).
    pub fn unwatch(&mut self) {
        // No-op: filesystem watching is not implemented yet.
    }

    /// Discover modules using the internally-set discoverer.
    pub async fn discover_internal(&mut self) -> Result<Vec<String>, ModuleError> {
        let discoverer = self.discoverer.as_ref().ok_or_else(|| {
            ModuleError::new(
                crate::errors::ErrorCode::ModuleLoadError,
                "No discoverer configured".to_string(),
            )
        })?;
        let discovered = discoverer.discover().await?;
        let mut registered_names = Vec::new();

        for dm in discovered {
            self.descriptors.insert(dm.name.clone(), dm.descriptor);
            self.lowercase_map
                .insert(dm.name.to_lowercase(), dm.name.clone());
            registered_names.push(dm.name);
        }

        Ok(registered_names)
    }

    /// Set the discoverer.
    pub fn set_discoverer(&mut self, discoverer: Box<dyn Discoverer>) {
        self.discoverer = Some(discoverer);
    }

    /// Set the validator.
    pub fn set_validator(&mut self, validator: Box<dyn ModuleValidator>) {
        self.validator = Some(validator);
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

    /// Export the combined input/output schema for a module.
    ///
    /// Returns the cached schema JSON, or `None` if the module is not registered.
    pub fn export_schema(&self, name: &str) -> Option<&serde_json::Value> {
        self.schema_cache.get(name)
    }

    /// Mark a module as disabled in its descriptor.
    ///
    /// Disabled modules remain registered but callers should check `is_enabled()`
    /// before dispatching. Returns an error if the module is not found.
    pub fn disable(&mut self, name: &str) -> Result<(), ModuleError> {
        let descriptor = self.descriptors.get_mut(name).ok_or_else(|| {
            ModuleError::new(
                crate::errors::ErrorCode::ModuleNotFound,
                format!("Module '{}' not found", name),
            )
        })?;
        descriptor.enabled = false;
        Ok(())
    }

    /// Mark a module as enabled in its descriptor.
    ///
    /// Returns an error if the module is not found.
    pub fn enable(&mut self, name: &str) -> Result<(), ModuleError> {
        let descriptor = self.descriptors.get_mut(name).ok_or_else(|| {
            ModuleError::new(
                crate::errors::ErrorCode::ModuleNotFound,
                format!("Module '{}' not found", name),
            )
        })?;
        descriptor.enabled = true;
        Ok(())
    }

    /// Return whether a module is enabled (per its descriptor).
    ///
    /// Returns `None` if the module is not registered.
    pub fn is_enabled(&self, name: &str) -> Option<bool> {
        self.descriptors.get(name).map(|d| d.enabled)
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

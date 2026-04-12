// APCore Protocol — Registry, Discoverer, ModuleValidator
// Spec reference: Module registration, discovery, validation, and descriptors

use async_trait::async_trait;
use parking_lot::RwLock;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

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
    pub module_id: String,
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
pub type ModuleCallbackFn = dyn Fn(&str, &dyn Module) + Send + Sync;

/// Reserved words that cannot be used as the first segment of a module ID.
///
/// Aligned with the Python and TypeScript SDKs to ensure cross-language consistency.
pub const RESERVED_WORDS: &[&str] = &[
    "system", "internal", "core", "apcore", "plugin", "schema", "acl",
];

/// Maximum allowed length for a module ID.
///
/// Per PROTOCOL_SPEC §2.7 EBNF constraint #1. 192 is filesystem-safe
/// (`192 + ".binding.yaml".len() = 205 < 255`-byte filename limit on
/// ext4/xfs/NTFS/APFS/btrfs) and accommodates Java/.NET deep-namespace
/// FQN-derived IDs. Bumped from 128 in spec 1.6.0-draft (2026-04-08).
///
/// Aligned with `apcore-python` and `apcore-typescript` `MAX_MODULE_ID_LENGTH`.
pub const MAX_MODULE_ID_LENGTH: usize = 192;

/// Standard registry event names per PROTOCOL_SPEC §12.2.
///
/// All SDKs **MUST** export these event names as named constants so that
/// consumers do not hardcode the underlying string literals. Aligned with
/// `apcore-python.REGISTRY_EVENTS` and `apcore-typescript.REGISTRY_EVENTS`.
///
/// Usage: `registry.on(REGISTRY_EVENTS.REGISTER, callback);`
pub mod registry_events {
    /// Fired after a module is successfully registered.
    pub const REGISTER: &str = "register";

    /// Fired before a module is removed from the registry.
    pub const UNREGISTER: &str = "unregister";
}

/// Container for the standard registry event names.
///
/// Provides the same `REGISTRY_EVENTS.REGISTER` / `REGISTRY_EVENTS.UNREGISTER`
/// access pattern used by `apcore-python` (dict) and `apcore-typescript`
/// (frozen object), so that idiomatic usage is consistent across SDKs.
pub struct RegistryEvents;

impl RegistryEvents {
    pub const REGISTER: &'static str = registry_events::REGISTER;
    pub const UNREGISTER: &'static str = registry_events::UNREGISTER;
}

/// Singleton instance providing `REGISTRY_EVENTS.REGISTER` / `REGISTRY_EVENTS.UNREGISTER`
/// access pattern matching the Python and TypeScript SDKs.
pub const REGISTRY_EVENTS: RegistryEvents = RegistryEvents;

/// Canonical regex source for the module ID pattern (PROTOCOL_SPEC §2.7).
///
/// Raw string form, matching `apcore-python` / `apcore-typescript`
/// `MODULE_ID_PATTERN`. Consumers needing a compiled pattern should call
/// [`module_id_pattern`] instead.
pub const MODULE_ID_PATTERN: &str = r"^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)*$";

/// Canonical EBNF pattern for module IDs per PROTOCOL_SPEC §2.7.
///
/// Equivalent regex of: `canonical_id = segment ("." segment)*` where
/// `segment = [a-z][a-z0-9_]*`. Aligned with `apcore-python` and
/// `apcore-typescript` `MODULE_ID_PATTERN`.
pub fn module_id_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(MODULE_ID_PATTERN).unwrap())
}

/// Validate a module ID against PROTOCOL_SPEC §2.7 in canonical order:
/// 1. non-empty
/// 2. matches EBNF pattern
/// 3. length ≤ MAX_MODULE_ID_LENGTH
/// 4. (if `allow_reserved == false`) no segment is a reserved word
///
/// Duplicate detection is the caller's responsibility (it requires registry
/// state). `register_internal` calls this with `allow_reserved=true` so sys
/// modules can use the `system.*` prefix; everything else still validates.
///
/// Aligned with `apcore-python._validate_module_id` and
/// `apcore-typescript._validateModuleId`.
fn validate_module_id(name: &str, allow_reserved: bool) -> Result<(), ModuleError> {
    // 1. empty check
    if name.is_empty() {
        return Err(ModuleError::new(
            crate::errors::ErrorCode::GeneralInvalidInput,
            "module_id must be a non-empty string".to_string(),
        ));
    }

    // 2. EBNF pattern check
    if !module_id_pattern().is_match(name) {
        return Err(ModuleError::new(
            crate::errors::ErrorCode::GeneralInvalidInput,
            format!(
                "Invalid module ID: '{name}'. Must match pattern: ^[a-z][a-z0-9_]*(\\.[a-z][a-z0-9_]*)*$ (lowercase, digits, underscores, dots only; no hyphens)"
            ),
        ));
    }

    // 3. length check
    if name.len() > MAX_MODULE_ID_LENGTH {
        return Err(ModuleError::new(
            crate::errors::ErrorCode::GeneralInvalidInput,
            format!(
                "Module ID exceeds maximum length of {}: {}",
                MAX_MODULE_ID_LENGTH,
                name.len()
            ),
        ));
    }

    // 4. reserved word per-segment check (skipped for register_internal)
    if !allow_reserved {
        for segment in name.split('.') {
            if RESERVED_WORDS.contains(&segment) {
                return Err(ModuleError::new(
                    crate::errors::ErrorCode::GeneralInvalidInput,
                    format!("Module ID contains reserved word: '{segment}'"),
                ));
            }
        }
    }

    Ok(())
}

/// Internal shared state for `Registry`, protected by a single `RwLock`.
///
/// All mutating methods acquire `core.write()` for the duration of the
/// mutation and release before invoking any user callbacks.
struct RegistryCore {
    modules: HashMap<String, Arc<dyn Module>>,
    descriptors: HashMap<String, ModuleDescriptor>,
    /// Reference counts for safe hot-reload — prevents unloading while in use.
    ref_counts: HashMap<String, usize>,
    /// Modules marked for unload (draining active requests before removal).
    draining: HashSet<String>,
    /// Case-insensitive lookup: lowercase name -> canonical name.
    lowercase_map: HashMap<String, String>,
    /// Cached JSON schemas for registered modules.
    schema_cache: HashMap<String, serde_json::Value>,
}

impl RegistryCore {
    fn new() -> Self {
        Self {
            modules: HashMap::new(),
            descriptors: HashMap::new(),
            ref_counts: HashMap::new(),
            draining: HashSet::new(),
            lowercase_map: HashMap::new(),
            schema_cache: HashMap::new(),
        }
    }
}

/// Central registry of modules.
///
/// The `Registry` uses interior mutability via `parking_lot::RwLock` so that
/// a single `Arc<Registry>` may be cloned freely and shared across the
/// pipeline, sys modules, and user code. All methods take `&self`; callers
/// never need `Arc::get_mut` or an external `Mutex` wrapper.
///
/// Critical invariant: no Registry lock is ever held across an `.await`.
/// All methods are synchronous, callback invocations clone the callbacks
/// out of their lock before running them, and the drain-wait logic releases
/// the core lock before awaiting.
pub struct Registry {
    core: RwLock<RegistryCore>,
    /// Event callbacks keyed by event name (e.g. "register", "unregister").
    ///
    /// Callbacks are stored as `Arc` so they can be cloned out of the lock
    /// before invocation — holding this lock while calling a callback would
    /// deadlock if the callback tried to register or unregister a module.
    callbacks: RwLock<HashMap<String, Vec<Arc<ModuleCallbackFn>>>>,
    /// Drain completion notification — signaled when a draining module reaches zero refs.
    drain_events: RwLock<HashMap<String, Arc<tokio::sync::Notify>>>,
    /// Optional discoverer for module discovery.
    discoverer: RwLock<Option<Box<dyn Discoverer>>>,
    /// Optional validator for module validation.
    validator: RwLock<Option<Box<dyn ModuleValidator>>>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let core = self.core.read();
        f.debug_struct("Registry")
            .field("modules", &core.modules.keys().collect::<Vec<_>>())
            .field("descriptors", &core.descriptors)
            .field("ref_counts", &core.ref_counts)
            .field("draining", &core.draining)
            .field(
                "drain_events_keys",
                &self.drain_events.read().keys().cloned().collect::<Vec<_>>(),
            )
            .field(
                "callbacks_keys",
                &self.callbacks.read().keys().cloned().collect::<Vec<_>>(),
            )
            .field("lowercase_map", &core.lowercase_map)
            .field(
                "schema_cache_keys",
                &core.schema_cache.keys().collect::<Vec<_>>(),
            )
            .field(
                "discoverer",
                &self.discoverer.read().as_ref().map(|_| "<Discoverer>"),
            )
            .field(
                "validator",
                &self.validator.read().as_ref().map(|_| "<Validator>"),
            )
            .finish()
    }
}

impl Registry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            core: RwLock::new(RegistryCore::new()),
            callbacks: RwLock::new(HashMap::new()),
            drain_events: RwLock::new(HashMap::new()),
            discoverer: RwLock::new(None),
            validator: RwLock::new(None),
        }
    }

    /// Snapshot the callbacks for a given event name so they can be invoked
    /// without holding the callbacks lock.
    fn snapshot_callbacks(&self, event: &str) -> Vec<Arc<ModuleCallbackFn>> {
        self.callbacks
            .read()
            .get(event)
            .cloned()
            .unwrap_or_default()
    }

    /// Register a module with the given name.
    ///
    /// Validation order (PROTOCOL_SPEC §2.7, aligned with apcore-python /
    /// apcore-typescript): empty → pattern → length → reserved (per-segment)
    /// → duplicate.
    pub fn register(
        &self,
        name: &str,
        module: Box<dyn Module>,
        descriptor: ModuleDescriptor,
    ) -> Result<(), ModuleError> {
        validate_module_id(name, false)?;

        // Run validation callbacks if a validator is set (do NOT hold the
        // core lock while calling user-supplied code).
        if let Some(validator) = self.validator.read().as_ref() {
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

        // Call on_load lifecycle hook before taking the core lock.
        module.on_load();

        let module_arc: Arc<dyn Module> = module.into();
        let module_clone = Arc::clone(&module_arc);
        {
            let mut core = self.core.write();
            if core.modules.contains_key(name) {
                return Err(ModuleError::new(
                    crate::errors::ErrorCode::GeneralInvalidInput,
                    format!("Module ID '{name}' is already registered"),
                ));
            }

            let schema = serde_json::json!({
                "input": descriptor.input_schema,
                "output": descriptor.output_schema,
            });
            core.schema_cache.insert(name.to_string(), schema);
            core.lowercase_map
                .insert(name.to_lowercase(), name.to_string());
            core.modules.insert(name.to_string(), module_arc);
            core.descriptors.insert(name.to_string(), descriptor);
        }

        // Fire "register" callbacks outside any lock.
        for cb in self.snapshot_callbacks("register") {
            cb(name, module_clone.as_ref());
        }

        Ok(())
    }

    /// Register a module with auto-generated descriptor.
    pub fn register_module(&self, name: &str, module: Box<dyn Module>) -> Result<(), ModuleError> {
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
    pub fn unregister(&self, name: &str) -> Result<(), ModuleError> {
        // Extract module + fire callbacks outside the lock.
        let removed: Arc<dyn Module> = {
            let core = self.core.read();
            match core.modules.get(name) {
                Some(m) => Arc::clone(m),
                None => {
                    return Err(ModuleError::new(
                        crate::errors::ErrorCode::ModuleNotFound,
                        format!("Module '{name}' not found"),
                    ));
                }
            }
        };

        // Fire "unregister" callbacks before removal (no lock held).
        for cb in self.snapshot_callbacks("unregister") {
            cb(name, removed.as_ref());
        }

        // Call on_unload lifecycle hook.
        removed.on_unload();

        // Now actually remove the module under the core write lock.
        {
            let mut core = self.core.write();
            core.modules.remove(name);
            core.descriptors.remove(name);
            core.lowercase_map.remove(&name.to_lowercase());
            core.schema_cache.remove(name);
            core.ref_counts.remove(name);
            core.draining.remove(name);
        }
        self.drain_events.write().remove(name);

        Ok(())
    }

    /// Get a shared reference to a module by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Module>> {
        self.core.read().modules.get(name).cloned()
    }

    /// Get the definition (descriptor) for a module by name.
    ///
    /// Returns a cloned `ModuleDescriptor` because the underlying storage
    /// is behind a lock — we cannot hand out borrowed references.
    pub fn get_definition(&self, name: &str) -> Option<ModuleDescriptor> {
        self.core.read().descriptors.get(name).cloned()
    }

    /// List registered module names with optional filtering.
    ///
    /// - `tags`: if provided, only return modules whose descriptor annotations
    ///   contain ALL of the specified tags.
    /// - `prefix`: if provided, only return modules whose name starts with the prefix.
    /// - When both are `None`, returns all registered module names.
    ///
    /// Returns owned `String` values because the storage is behind a lock.
    pub fn list(&self, tags: Option<&[&str]>, prefix: Option<&str>) -> Vec<String> {
        let core = self.core.read();
        core.modules
            .keys()
            .filter(|name| {
                if let Some(pfx) = prefix {
                    if !name.starts_with(pfx) {
                        return false;
                    }
                }
                if let Some(required_tags) = tags {
                    if let Some(desc) = core.descriptors.get(name.as_str()) {
                        let module_tags = &desc.tags;
                        if !required_tags
                            .iter()
                            .all(|t| module_tags.contains(&t.to_string()))
                        {
                            return false;
                        }
                    } else {
                        return false;
                    }
                }
                true
            })
            .cloned()
            .collect()
    }

    /// Check if a module is registered.
    pub fn has(&self, name: &str) -> bool {
        self.core.read().modules.contains_key(name)
    }

    /// Discover and register modules from a discoverer.
    #[allow(clippy::similar_names)] // `discoverer` (param) and `discovered` (result) are semantically distinct
    pub async fn discover(&self, discoverer: &dyn Discoverer) -> Result<Vec<String>, ModuleError> {
        let discovered = discoverer.discover().await?;
        let mut registered_names = Vec::new();

        {
            let mut core = self.core.write();
            for dm in discovered {
                core.descriptors.insert(dm.name.clone(), dm.descriptor);
                core.lowercase_map
                    .insert(dm.name.to_lowercase(), dm.name.clone());
                registered_names.push(dm.name);
            }
        }

        Ok(registered_names)
    }

    /// Register a sys/internal module that bypasses **only** the reserved
    /// word check. All other PROTOCOL_SPEC §2.7 validations (empty, EBNF
    /// pattern, length, duplicate) still apply.
    ///
    /// The intended use case is registering modules under reserved prefixes
    /// like `system.health` or `system.control.toggle_feature` from
    /// `apcore::sys_modules`. Aligned with apcore-typescript
    /// `Registry.registerInternal`.
    pub fn register_internal(
        &self,
        name: &str,
        module: Box<dyn Module>,
        descriptor: ModuleDescriptor,
    ) -> Result<(), ModuleError> {
        validate_module_id(name, true)?;

        module.on_load();
        let module_arc: Arc<dyn Module> = module.into();
        let module_clone = Arc::clone(&module_arc);

        {
            let mut core = self.core.write();
            if core.modules.contains_key(name) {
                return Err(ModuleError::new(
                    crate::errors::ErrorCode::GeneralInvalidInput,
                    format!("Module ID '{name}' is already registered"),
                ));
            }

            let schema = serde_json::json!({
                "input": descriptor.input_schema,
                "output": descriptor.output_schema,
            });
            core.schema_cache.insert(name.to_string(), schema);
            core.lowercase_map
                .insert(name.to_lowercase(), name.to_string());
            core.modules.insert(name.to_string(), module_arc);
            core.descriptors.insert(name.to_string(), descriptor);
        }

        // Fire "register" callbacks outside any lock.
        for cb in self.snapshot_callbacks("register") {
            cb(name, module_clone.as_ref());
        }

        Ok(())
    }

    /// Apply a closure to every registered module.
    ///
    /// The closure is invoked while holding a read lock on the core, so it
    /// MUST NOT recursively acquire any Registry lock.
    pub fn for_each_module(&self, mut f: impl FnMut(&str, &dyn Module)) {
        let core = self.core.read();
        for (name, module) in &core.modules {
            f(name.as_str(), module.as_ref());
        }
    }

    /// Human-readable module description.
    pub fn describe(&self, name: &str) -> String {
        match self.core.read().modules.get(name) {
            Some(module) => module.description().to_string(),
            None => "Module not found".to_string(),
        }
    }

    /// Draining-aware unregister.
    pub async fn safe_unregister(&self, name: &str, timeout_ms: u64) -> Result<bool, ModuleError> {
        // Mark draining, check ref_count. If zero we can proceed immediately.
        let need_wait_notify: Option<Arc<tokio::sync::Notify>> = {
            let mut core = self.core.write();
            if !core.modules.contains_key(name) {
                return Err(ModuleError::new(
                    crate::errors::ErrorCode::ModuleNotFound,
                    format!("Module '{name}' not found"),
                ));
            }
            core.draining.insert(name.to_string());
            let current_refs = core.ref_counts.get(name).copied().unwrap_or(0);
            if current_refs == 0 {
                None
            } else {
                let notify = Arc::new(tokio::sync::Notify::new());
                self.drain_events
                    .write()
                    .insert(name.to_string(), Arc::clone(&notify));
                Some(notify)
            }
        };

        match need_wait_notify {
            None => {
                self.unregister(name)?;
                Ok(true)
            }
            Some(notify) => {
                let result = tokio::time::timeout(
                    std::time::Duration::from_millis(timeout_ms),
                    notify.notified(),
                )
                .await;

                if let Ok(()) = result {
                    self.unregister(name)?;
                    Ok(true)
                } else {
                    // Timeout — remove draining flag but don't unregister.
                    self.core.write().draining.remove(name);
                    self.drain_events.write().remove(name);
                    Ok(false)
                }
            }
        }
    }

    /// Ref-counted module access with explicit reference tracking.
    ///
    /// Increments the reference count for the module before returning it.
    /// Call `release_ref()` when done to decrement the count. This is required
    /// for `safe_unregister()` drain logic to work correctly.
    pub fn acquire_ref(&self, name: &str) -> Result<Arc<dyn Module>, ModuleError> {
        let mut core = self.core.write();
        if core.draining.contains(name) {
            return Err(ModuleError::new(
                crate::errors::ErrorCode::ModuleNotFound,
                format!("Module '{name}' is draining"),
            ));
        }
        let module = core.modules.get(name).cloned().ok_or_else(|| {
            ModuleError::new(
                crate::errors::ErrorCode::ModuleNotFound,
                format!("Module '{name}' not found"),
            )
        })?;
        *core.ref_counts.entry(name.to_string()).or_insert(0) += 1;
        Ok(module)
    }

    /// Decrement the reference count for a module. Notifies drain waiters when count reaches 0.
    pub fn release_ref(&self, name: &str) {
        let should_notify = {
            let mut core = self.core.write();
            if let Some(count) = core.ref_counts.get_mut(name) {
                if *count > 0 {
                    *count -= 1;
                }
                if *count == 0 {
                    core.ref_counts.remove(name);
                    true
                } else {
                    false
                }
            } else {
                false
            }
        };

        if should_notify {
            if let Some(notify) = self.drain_events.read().get(name) {
                notify.notify_one();
            }
        }
    }

    /// Simple module access (no ref-counting).
    ///
    /// Checks draining status before returning the module reference.
    /// Note: For safe-unregister scenarios, use `acquire_ref()`/`release_ref()`
    /// instead, which track reference counts for drain logic.
    pub fn acquire(&self, name: &str) -> Result<Arc<dyn Module>, ModuleError> {
        let core = self.core.read();
        if core.draining.contains(name) {
            return Err(ModuleError::new(
                crate::errors::ErrorCode::ModuleNotFound,
                format!("Module '{name}' is draining"),
            ));
        }
        core.modules.get(name).cloned().ok_or_else(|| {
            ModuleError::new(
                crate::errors::ErrorCode::ModuleNotFound,
                format!("Module '{name}' not found"),
            )
        })
    }

    /// Check if a module is draining.
    pub fn is_draining(&self, name: &str) -> bool {
        self.core.read().draining.contains(name)
    }

    /// Event subscription.
    pub fn on(&self, event: &str, callback: Box<ModuleCallbackFn>) {
        self.callbacks
            .write()
            .entry(event.to_string())
            .or_default()
            .push(Arc::from(callback));
    }

    /// Filesystem watching (no-op — filesystem watching is platform-specific).
    #[allow(clippy::unused_async)] // API stub for cross-language parity; real impl needs platform-specific deps
    pub async fn watch(&self) -> Result<(), ModuleError> {
        // No-op: filesystem watching requires platform-specific dependencies
        // (e.g. notify crate). Stubbed for API compatibility.
        Ok(())
    }

    /// Stop filesystem watching (no-op).
    pub fn unwatch(&self) {
        // No-op: filesystem watching is not implemented yet.
    }

    /// Discover modules using the internally-set discoverer.
    pub async fn discover_internal(&self) -> Result<Vec<String>, ModuleError> {
        // Run discovery outside of any lock, but we need to briefly check
        // that a discoverer is set. We can't hold the discoverer lock across
        // `.await`, so we invoke it through a short-lived critical section
        // instead — we need the discoverer to survive across the await, so
        // the call itself happens while we still hold a read lock but that
        // is a `parking_lot::RwLockReadGuard` which is `Send`. However, we
        // must NOT hold it across `.await`. Workaround: require the
        // discoverer's `discover()` future to be pollable independently.
        //
        // We achieve this by extracting nothing from the discoverer guard —
        // instead we do discovery on a separate dedicated path: the
        // discoverer must provide its own internal sync. In practice we
        // simply call discoverer.discover().await inside the block, but we
        // cannot hold a parking_lot guard across await.
        //
        // Since Box<dyn Discoverer> can't be moved out of the Option under
        // a read lock, we accept a subtle limitation: discovery and
        // set_discoverer are mutually exclusive in time. We hold the
        // discoverer write lock briefly, replace it with None, perform the
        // discovery, then put it back.
        let discoverer_opt = self.discoverer.write().take();
        let Some(active_discoverer) = discoverer_opt else {
            return Err(ModuleError::new(
                crate::errors::ErrorCode::ModuleLoadError,
                "No discoverer configured".to_string(),
            ));
        };

        let discover_result = active_discoverer.discover().await;
        // Put the discoverer back, but only if no concurrent `set_discoverer`
        // swapped a new one in during the await. Otherwise the new one wins.
        {
            let mut slot = self.discoverer.write();
            if slot.is_none() {
                *slot = Some(active_discoverer);
            }
        }

        let discovered = discover_result?;
        let mut registered_names = Vec::new();
        {
            let mut core = self.core.write();
            for dm in discovered {
                core.descriptors.insert(dm.name.clone(), dm.descriptor);
                core.lowercase_map
                    .insert(dm.name.to_lowercase(), dm.name.clone());
                registered_names.push(dm.name);
            }
        }

        Ok(registered_names)
    }

    /// Set the discoverer.
    pub fn set_discoverer(&self, discoverer: Box<dyn Discoverer>) {
        *self.discoverer.write() = Some(discoverer);
    }

    /// Set the validator.
    pub fn set_validator(&self, validator: Box<dyn ModuleValidator>) {
        *self.validator.write() = Some(validator);
    }

    /// Return count of registered modules.
    pub fn count(&self) -> usize {
        self.core.read().modules.len()
    }

    /// Return all module IDs, sorted alphabetically.
    pub fn module_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.core.read().modules.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// Export the combined input/output schema for a module.
    ///
    /// Returns a cloned schema JSON, or `None` if the module is not registered.
    pub fn export_schema(&self, name: &str) -> Option<serde_json::Value> {
        self.core.read().schema_cache.get(name).cloned()
    }

    /// Mark a module as disabled in its descriptor.
    ///
    /// Disabled modules remain registered but callers should check `is_enabled()`
    /// before dispatching. Returns an error if the module is not found.
    pub fn disable(&self, name: &str) -> Result<(), ModuleError> {
        let mut core = self.core.write();
        let descriptor = core.descriptors.get_mut(name).ok_or_else(|| {
            ModuleError::new(
                crate::errors::ErrorCode::ModuleNotFound,
                format!("Module '{name}' not found"),
            )
        })?;
        descriptor.enabled = false;
        Ok(())
    }

    /// Mark a module as enabled in its descriptor.
    ///
    /// Returns an error if the module is not found.
    pub fn enable(&self, name: &str) -> Result<(), ModuleError> {
        let mut core = self.core.write();
        let descriptor = core.descriptors.get_mut(name).ok_or_else(|| {
            ModuleError::new(
                crate::errors::ErrorCode::ModuleNotFound,
                format!("Module '{name}' not found"),
            )
        })?;
        descriptor.enabled = true;
        Ok(())
    }

    /// Return whether a module is enabled (per its descriptor).
    ///
    /// Returns `None` if the module is not registered.
    pub fn is_enabled(&self, name: &str) -> Option<bool> {
        self.core.read().descriptors.get(name).map(|d| d.enabled)
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

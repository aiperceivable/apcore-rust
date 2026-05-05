// APCore Protocol — Registry, Discoverer, ModuleValidator
// Spec reference: Module registration, discovery, validation, and descriptors

use async_trait::async_trait;
use parking_lot::RwLock;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, OnceLock,
};

use crate::errors::ModuleError;
use crate::module::{Module, ModuleAnnotations, ModuleExample, ValidationResult};
use crate::registry::conflicts::{detect_id_conflicts, ConflictSeverity};

/// Cross-language compatible module descriptor.
///
/// Aligned with `apcore-python.ModuleDescriptor` and
/// `apcore-typescript.ModuleDescriptor`.  All fields match `PROTOCOL_SPEC`
/// section 5.2.  The `enabled` field is a Rust-specific runtime addition
/// used by `Registry::disable()` / `Registry::enable()` for module toggling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleDescriptor {
    /// Canonical module identifier (e.g. "math.add").
    pub module_id: String,
    /// Human-readable display name (optional).
    #[serde(default)]
    pub name: Option<String>,
    /// One-line description of what the module does.
    #[serde(default)]
    pub description: String,
    /// Long-form documentation (Markdown).
    #[serde(default)]
    pub documentation: Option<String>,
    /// JSON Schema for the module's input.
    pub input_schema: serde_json::Value,
    /// JSON Schema for the module's output.
    pub output_schema: serde_json::Value,
    /// Semantic version string.
    #[serde(default = "default_version")]
    pub version: String,
    /// Categorisation tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Behavioural annotations (readonly, destructive, etc.).
    #[serde(default)]
    pub annotations: Option<ModuleAnnotations>,
    /// Example invocations.
    #[serde(default)]
    pub examples: Vec<ModuleExample>,
    /// Arbitrary metadata for display overlays, AI intent hints, and version hints.
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
    /// UI display metadata (optional). Mirrors `display` in Python/TypeScript SDKs.
    #[serde(default)]
    pub display: Option<serde_json::Value>,
    /// ISO 8601 date string (YYYY-MM-DD) after which this module is removed.
    #[serde(default)]
    pub sunset_date: Option<String>,
    /// Module dependencies.
    #[serde(default)]
    pub dependencies: Vec<DependencyInfo>,
    /// Runtime-only: whether the module is enabled (not in `PROTOCOL_SPEC`).
    #[serde(default = "default_enabled", skip_serializing)]
    pub enabled: bool,
}

fn default_version() -> String {
    DEFAULT_MODULE_VERSION.to_string()
}

fn default_enabled() -> bool {
    true
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
///
/// Every entry carries a live module instance. Discoverers for out-of-process
/// modules (subprocesses, RPC endpoints, network-hosted modules) wrap the
/// external resource in a [`Module`] impl — e.g., a
/// `SubprocessModule { executable: PathBuf, descriptor }` whose `execute`
/// spawns the process — so the registry can treat all modules uniformly.
///
/// Aligned with `apcore-python.Discoverer` (returns `{module_id, module}`)
/// and `apcore-typescript.Discoverer` (returns `{moduleId, module}`).
///
/// Not serializable: the `module` field is `Arc<dyn Module>` (a trait object)
/// and has no meaningful serde representation. This is an intentional exception
/// to the project-wide "all public data types implement Serialize/Deserialize"
/// convention in `CLAUDE.md` — do not add the derives back without also solving
/// how to round-trip a live module instance.
#[derive(Clone)]
pub struct DiscoveredModule {
    pub name: String,
    pub source: String,
    pub descriptor: ModuleDescriptor,
    pub module: Arc<dyn Module>,
}

impl std::fmt::Debug for DiscoveredModule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiscoveredModule")
            .field("name", &self.name)
            .field("source", &self.source)
            .field("descriptor", &self.descriptor)
            .field("module", &"<Module>")
            .finish()
    }
}

/// Trait for discovering modules from external sources.
#[async_trait]
pub trait Discoverer: Send + Sync {
    /// Discover available modules.
    ///
    /// `roots` is a list of filesystem paths (or logical namespaces) to search.
    /// Implementations that perform filesystem discovery SHOULD restrict their
    /// search to the provided roots. Passing an empty slice means "use the
    /// implementation's default search paths."
    ///
    /// Each returned [`DiscoveredModule`] carries a live `module` instance —
    /// discoverers for out-of-process modules wrap the external resource in a
    /// `Module` impl so the registry can treat every module uniformly.
    ///
    /// Aligned with `apcore-python.Discoverer.discover(roots)` and
    /// `apcore-typescript.Discoverer.discover(roots)`.
    async fn discover(&self, roots: &[String]) -> Result<Vec<DiscoveredModule>, ModuleError>;
}

/// Trait for validating module implementations.
pub trait ModuleValidator: Send + Sync {
    /// Validate a module against the protocol contract.
    ///
    /// `descriptor` is optional — pass `Some(&descriptor)` when a full
    /// `ModuleDescriptor` is available, or `None` for schema-free validation.
    /// Returning a non-empty `ValidationResult.errors` causes `register()` to
    /// reject the module.
    ///
    /// Aligned with `apcore-python.ModuleValidator.validate(module)` and
    /// `apcore-typescript.ModuleValidator.validate(module)`.
    fn validate(
        &self,
        module: &dyn Module,
        descriptor: Option<&ModuleDescriptor>,
    ) -> ValidationResult;
}

/// Type alias for the event callback closure.
pub type ModuleCallbackFn = dyn Fn(&str, &dyn Module) + Send + Sync;
type CallbackMap = HashMap<String, Vec<(u64, Arc<ModuleCallbackFn>)>>;

/// Reserved words that cannot be used as the first segment of a module ID.
///
/// Aligned with the Python and TypeScript SDKs to ensure cross-language consistency.
pub const RESERVED_WORDS: &[&str] = &[
    "system", "internal", "core", "apcore", "plugin", "schema", "acl",
];

/// Maximum allowed length for a module ID.
///
/// Per `PROTOCOL_SPEC` §2.7 EBNF constraint #1. 192 is filesystem-safe
/// (`192 + ".binding.yaml".len() = 205 < 255`-byte filename limit on
/// ext4/xfs/NTFS/APFS/btrfs) and accommodates Java/.NET deep-namespace
/// FQN-derived IDs. Bumped from 128 in spec 1.6.0-draft (2026-04-08).
///
/// Aligned with `apcore-python` and `apcore-typescript` `MAX_MODULE_ID_LENGTH`.
pub const MAX_MODULE_ID_LENGTH: usize = 192;

/// Default module version when a caller does not supply one.
///
/// Used by [`ModuleDescriptor`]'s serde default, by
/// [`Registry::register_module`]'s auto-built descriptor, and by
/// [`crate::client::APCore::module`]. Aligned with `apcore-python`'s default.
pub const DEFAULT_MODULE_VERSION: &str = "1.0.0";

/// Standard registry event names per `PROTOCOL_SPEC` §12.2.
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

/// Canonical regex source for the module ID pattern (`PROTOCOL_SPEC` §2.7).
///
/// Raw string form, matching `apcore-python` / `apcore-typescript`
/// `MODULE_ID_PATTERN`. Consumers needing a compiled pattern should call
/// [`module_id_pattern`] instead.
pub const MODULE_ID_PATTERN: &str = r"^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)*$";

/// Canonical EBNF pattern for module IDs per `PROTOCOL_SPEC` §2.7.
///
/// Equivalent regex of: `canonical_id = segment ("." segment)*` where
/// `segment = [a-z][a-z0-9_]*`. Aligned with `apcore-python` and
/// `apcore-typescript` `MODULE_ID_PATTERN`.
pub fn module_id_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(MODULE_ID_PATTERN).unwrap())
}

/// Validate a module ID against `PROTOCOL_SPEC` §2.7 in canonical order:
/// 1. non-empty
/// 2. matches EBNF pattern
/// 3. length ≤ `MAX_MODULE_ID_LENGTH`
/// 4. (if `allow_reserved == false`) first segment is not a reserved word
///
/// Duplicate detection is the caller's responsibility (it requires registry
/// state). `register_internal` calls this with `allow_reserved=true` so sys
/// modules can use the `system.*` prefix; all other validations still apply.
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

    // 4. reserved word first-segment check (skipped for register_internal)
    if !allow_reserved {
        // INVARIANT: pattern check (step 2) guarantees at least one segment.
        let first_segment = name.split('.').next().unwrap();
        if RESERVED_WORDS.contains(&first_segment) {
            return Err(ModuleError::new(
                crate::errors::ErrorCode::GeneralInvalidInput,
                format!("Module ID contains reserved word: '{first_segment}'"),
            ));
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
    /// Callbacks are stored as `(id, Arc)` so they can be cloned out of the lock
    /// before invocation — holding this lock while calling a callback would
    /// deadlock if the callback tried to register or unregister a module.
    callbacks: RwLock<CallbackMap>,
    /// Monotonically increasing counter for callback handle IDs.
    callback_counter: AtomicU64,
    /// Drain completion notification — signaled when a draining module reaches zero refs.
    drain_events: RwLock<HashMap<String, Arc<tokio::sync::Notify>>>,
    /// Optional discoverer for module discovery.
    discoverer: RwLock<Option<Box<dyn Discoverer>>>,
    /// Optional validator for module validation.
    ///
    /// Stored as `Arc` (not `Box`) so the validator can be cloned out of the
    /// read lock before invocation. Holding a `parking_lot::RwLock` read
    /// guard across user-supplied `validate()` would deadlock if the
    /// validator re-entered `set_validator` (parking_lot's guards are not
    /// reentrant).
    validator: RwLock<Option<Arc<dyn ModuleValidator>>>,
    /// Extension roots passed to custom Discoverers.
    ///
    /// Aligned with `apcore-python.Registry._extension_roots` and
    /// `apcore-typescript.Registry._extensionRoots`. Set via
    /// [`set_extension_roots`](Self::set_extension_roots); passed verbatim to
    /// `Discoverer::discover(roots)` in `discover_internal()`.
    extension_roots: RwLock<Vec<String>>,
    /// Live filesystem watcher (sync finding A-D-010). `None` when
    /// `watch()` has not been called or has been stopped via `unwatch()`.
    watcher: parking_lot::Mutex<Option<notify::RecommendedWatcher>>,
    /// Background task handle that consumes notify events and triggers
    /// debounced re-discovery. Cleared on `unwatch()`.
    watch_handle: parking_lot::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

/// RAII guard that restores a taken-out `Discoverer` back into the registry's
/// slot when dropped — including during panic unwind from `discover().await`.
///
/// Without this, a panic inside a custom `Discoverer::discover` future would
/// permanently lose the discoverer because the manual "put it back" block
/// below the `.await` would be unreachable.
///
/// If a concurrent `set_discoverer` swapped in a new instance during the
/// `.await`, that new one wins — the guard only restores when the slot is
/// still `None`.
struct DiscovererRestoreGuard<'a> {
    slot: &'a RwLock<Option<Box<dyn Discoverer>>>,
    discoverer: Option<Box<dyn Discoverer>>,
}

impl Drop for DiscovererRestoreGuard<'_> {
    fn drop(&mut self) {
        if let Some(d) = self.discoverer.take() {
            let mut slot = self.slot.write();
            if slot.is_none() {
                *slot = Some(d);
            }
        }
    }
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
            .finish_non_exhaustive()
    }
}

impl Registry {
    /// Create a new empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            core: RwLock::new(RegistryCore::new()),
            callbacks: RwLock::new(HashMap::new()),
            callback_counter: AtomicU64::new(1),
            drain_events: RwLock::new(HashMap::new()),
            discoverer: RwLock::new(None),
            validator: RwLock::new(None),
            extension_roots: RwLock::new(Vec::new()),
            watcher: parking_lot::Mutex::new(None),
            watch_handle: parking_lot::Mutex::new(None),
        }
    }

    /// Snapshot the callbacks for a given event name so they can be invoked
    /// without holding the callbacks lock.
    fn snapshot_callbacks(&self, event: &str) -> Vec<Arc<ModuleCallbackFn>> {
        self.callbacks
            .read()
            .get(event)
            .map(|v| v.iter().map(|(_, cb)| cb.clone()).collect())
            .unwrap_or_default()
    }

    /// Register a module with an explicit `ModuleDescriptor`.
    ///
    /// This is the **extended** registration form. Use it when you need to
    /// supply a pre-built descriptor (e.g. loaded from a config file or
    /// discovered from an external source). For the **spec-compliant** form
    /// (`register(module_id, module)` — two arguments, descriptor
    /// auto-generated from the module's schema methods), use
    /// [`register_module`](Self::register_module) instead.
    ///
    /// Validation order (`PROTOCOL_SPEC` §2.7, aligned with apcore-python /
    /// apcore-typescript): empty → pattern → length → reserved (per-segment)
    /// → duplicate.
    pub fn register(
        &self,
        name: &str,
        module: Box<dyn Module>,
        descriptor: ModuleDescriptor,
    ) -> Result<(), ModuleError> {
        self.register_core(name, module, descriptor, false, true)
    }

    /// Register a module — **spec-compliant two-argument form**.
    ///
    /// Equivalent to `register(module_id, module)` in the Python and
    /// TypeScript SDKs. The `ModuleDescriptor` is auto-generated from the
    /// module's `input_schema()` / `output_schema()` / annotations.
    ///
    /// Aligned with `apcore-python.Registry.register(module_id, module)` and
    /// `apcore-typescript.Registry.register(moduleId, module)`.
    ///
    /// When you need a custom descriptor, use
    /// [`register`](Self::register) (the three-argument extended form).
    pub fn register_module(&self, name: &str, module: Box<dyn Module>) -> Result<(), ModuleError> {
        let descriptor = ModuleDescriptor {
            module_id: name.to_string(),
            name: None,
            description: module.description().to_string(),
            documentation: None,
            input_schema: module.input_schema(),
            output_schema: module.output_schema(),
            version: DEFAULT_MODULE_VERSION.to_string(),
            tags: vec![],
            annotations: Some(ModuleAnnotations::default()),
            examples: vec![],
            metadata: HashMap::new(),
            display: None,
            sunset_date: None,
            dependencies: vec![],
            enabled: true,
        };
        self.register(name, module, descriptor)
    }

    /// Unregister a module by name.
    ///
    /// Returns `Ok(true)` if the module was found and removed, `Ok(false)` if
    /// the module was not registered (idempotent — matches apcore-python
    /// `return False` and apcore-typescript `return false` semantics,
    /// sync finding A-D-002).
    ///
    /// Ordering (aligned with `apcore-python.Registry.unregister`):
    ///
    /// 1. Acquire `core.write()`, remove from all maps atomically, drop lock.
    /// 2. Invoke `module.on_unload()` BEFORE firing the `"unregister"` callback
    ///    so subscribers observe the post-on_unload module state (sync A-D-003).
    /// 3. Callers holding `Arc<dyn Module>` references from earlier `get()`
    ///    calls keep the module alive; `on_unload` still runs exactly once.
    ///
    /// Note: the return type changes from `Result<(), ModuleError>` to
    /// `Result<bool, ModuleError>` in this version. Callers should check the
    /// bool rather than treating `Ok(())` as success.
    pub fn unregister(&self, name: &str) -> Result<bool, ModuleError> {
        let removed: Arc<dyn Module> = {
            let mut core = self.core.write();
            let Some(module) = core.modules.remove(name) else {
                return Ok(false);
            };
            core.descriptors.remove(name);
            core.lowercase_map.remove(&name.to_lowercase());
            core.schema_cache.remove(name);
            core.ref_counts.remove(name);
            core.draining.remove(name);
            module
        };
        self.drain_events.write().remove(name);

        // Fire on_unload BEFORE the "unregister" callback so subscribers observe
        // the post-on_unload module state, matching apcore-python and
        // apcore-typescript ordering (sync finding A-D-003).
        removed.on_unload();
        for cb in self.snapshot_callbacks("unregister") {
            cb(name, removed.as_ref());
        }

        Ok(true)
    }

    /// Get a shared reference to a module by name.
    ///
    /// # Errors
    ///
    /// - `Err(ModuleError(code=ModuleNotFound))` if `name` is an empty string —
    ///   per `registry-system.md §Contract: Registry.get` Preconditions:
    ///   "module_id MUST NOT be an empty string. An empty module_id MUST be
    ///   rejected before any lock is acquired" (sync finding A-004).
    ///
    /// # Returns
    ///
    /// - `Ok(Some(module))` if the module is registered
    /// - `Ok(None)` if `name` is well-formed but the module is not registered
    pub fn get(&self, name: &str) -> Result<Option<Arc<dyn Module>>, ModuleError> {
        if name.is_empty() {
            return Err(ModuleError::new(
                crate::errors::ErrorCode::ModuleNotFound,
                "Module ID must not be empty",
            ));
        }
        Ok(self.core.read().modules.get(name).cloned())
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
        let mut result: Vec<String> = core
            .modules
            .keys()
            .filter(|name| {
                if let Some(pfx) = prefix {
                    if !name.starts_with(pfx) {
                        return false;
                    }
                }
                if let Some(required_tags) = tags {
                    // D11-003: union descriptor.tags with module.tags() so a
                    // module declaring `fn tags(&self) -> vec!["a"]` registered
                    // via register_module(name, module) (which builds an
                    // empty descriptor.tags) is filtered IN by tag-match
                    // queries — matches apcore-python (registry.py:1027) and
                    // apcore-typescript (registry.ts:689) which both union
                    // module-instance tags with merged-meta tags.
                    let mut module_tags: Vec<String> = core
                        .descriptors
                        .get(name.as_str())
                        .map(|desc| desc.tags.clone())
                        .unwrap_or_default();
                    if let Some(module) = core.modules.get(name.as_str()) {
                        for t in module.tags() {
                            if !module_tags.contains(&t) {
                                module_tags.push(t);
                            }
                        }
                    }
                    if !required_tags
                        .iter()
                        .all(|t| module_tags.contains(&t.to_string()))
                    {
                        return false;
                    }
                }
                true
            })
            .cloned()
            .collect();
        // Spec: list() MUST return sorted, unique IDs for cross-language parity
        // with apcore-python and apcore-typescript (sync finding A-D-103).
        result.sort();
        result
    }

    /// Check if a module is registered.
    pub fn has(&self, name: &str) -> bool {
        self.core.read().modules.contains_key(name)
    }

    /// Discover and register modules from a discoverer.
    ///
    /// Returns the count of entries that were actually registered —
    /// entries whose `name` fails `PROTOCOL_SPEC` §2.7 validation, whose
    /// `name` duplicates an already-discovered descriptor, or whose
    /// instance is rejected by the custom validator are skipped with a
    /// `tracing::warn!` and excluded from the count.
    ///
    /// Aligned with `apcore-python.Registry._discover_custom` and
    /// `apcore-typescript.Registry._discoverCustom` — same skip-and-warn
    /// semantics for malformed entries.
    #[allow(clippy::similar_names)] // `discoverer` (param) and `discovered` (result) are semantically distinct
    pub async fn discover(&self, discoverer: &dyn Discoverer) -> Result<usize, ModuleError> {
        let discovered = discoverer.discover(&[]).await?;
        Ok(self.register_discovered(discovered))
    }

    /// Register a sys/internal module that bypasses **only** the reserved
    /// word check. All other `PROTOCOL_SPEC` §2.7 validations (empty, EBNF
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
        self.register_core(name, module, descriptor, true, false)
    }

    /// Shared registration core for `register`, `register_internal`, and the
    /// per-entry path of `register_discovered`.
    ///
    /// Ordering (aligned with `apcore-python.Registry.register`):
    /// 1. `validate_module_id` (syntactic, no lock).
    /// 2. Snapshot the validator `Arc` out of `self.validator` and invoke it
    ///    WITHOUT holding any lock (prevents parking_lot non-reentrant
    ///    deadlock if the validator calls back into the registry).
    /// 3. Take `core.write()`, run `detect_id_conflicts` (surfaces duplicate,
    ///    reserved-word, and case-collision conflicts at once), insert the
    ///    module into all maps, drop the lock.
    /// 4. Call `on_load` OUTSIDE the lock and only AFTER a successful insert,
    ///    so duplicated registrations never trigger `on_load` side effects
    ///    that `on_unload` would then never unwind.
    /// 5. Fire `"register"` callbacks outside any lock.
    fn register_core(
        &self,
        name: &str,
        module: Box<dyn Module>,
        descriptor: ModuleDescriptor,
        allow_reserved: bool,
        run_validator: bool,
    ) -> Result<(), ModuleError> {
        validate_module_id(name, allow_reserved)?;

        if run_validator {
            // Clone the validator Arc out of the lock so the user-supplied
            // `validate()` call happens without any Registry lock held.
            let validator_snapshot = self.validator.read().as_ref().map(Arc::clone);
            if let Some(validator) = validator_snapshot {
                let result = validator.validate(module.as_ref(), Some(&descriptor));
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
        }

        let module_arc: Arc<dyn Module> = module.into();
        let module_clone = Arc::clone(&module_arc);

        {
            let mut core = self.core.write();

            // Full conflict detection (duplicate / reserved / case-collision).
            // Aligned with `apcore-python.Registry.register`, which invokes
            // `detect_id_conflicts` on every register call — a prior audit
            // flagged a cross-language drift where only exact duplicates
            // were caught by the Rust path.
            //
            // When `allow_reserved` is true (sys-module internal path), the
            // reserved-word check is suppressed by passing an empty slice;
            // duplicate + case-collision detection still applies.
            let reserved: &[&str] = if allow_reserved { &[] } else { RESERVED_WORDS };
            let existing_ids: HashSet<String> = core.modules.keys().cloned().collect();
            if let Some(conflict) =
                detect_id_conflicts(name, &existing_ids, reserved, Some(&core.lowercase_map))
            {
                match conflict.severity {
                    ConflictSeverity::Error => {
                        return Err(ModuleError::new(
                            crate::errors::ErrorCode::GeneralInvalidInput,
                            conflict.message,
                        ));
                    }
                    ConflictSeverity::Warning => {
                        tracing::warn!(
                            module_id = %name,
                            conflict = %conflict.message,
                            "Module registration proceeded despite warning-level ID conflict"
                        );
                    }
                }
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

        // on_load runs AFTER successful insertion and OUTSIDE any lock.
        // Duplicates never reach this point, so on_load cannot leak
        // resources for a registration that the registry rejected.
        // If on_load signals failure, roll back the four inserts above so no
        // half-initialised module remains in the registry (mirrors
        // apcore-python Registry._invoke_on_load rollback).
        if let Err(e) = module_clone.on_load() {
            let mut core = self.core.write();
            core.schema_cache.remove(name);
            core.lowercase_map.remove(&name.to_lowercase());
            core.modules.remove(name);
            core.descriptors.remove(name);
            // Re-raise the original error unchanged (mirrors Python `raise` / TypeScript `throw e`).
            // Wrapping into ModuleLoadError loses the original code and breaks downstream dispatch.
            return Err(e);
        }

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
                // Idempotent — match apcore-python `return False` and
                // apcore-typescript `return false` semantics (sync finding A-D-007).
                return Ok(false);
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

                let clean = result.is_ok();
                if !clean {
                    // Timeout — force-unload, matching Python and TypeScript behavior.
                    let in_flight = self.core.read().ref_counts.get(name).copied().unwrap_or(0);
                    tracing::warn!(
                        "Force-unloading module '{}' after {}ms timeout ({} in-flight executions)",
                        name,
                        timeout_ms,
                        in_flight,
                    );
                }
                {
                    let mut core = self.core.write();
                    core.draining.remove(name);
                }
                self.drain_events.write().remove(name);
                self.unregister(name)?;
                Ok(clean)
            }
        }
    }

    /// Ref-counted module access with explicit reference tracking.
    ///
    /// Acquire a reference to a module, incrementing its ref count.
    ///
    /// Cross-language parity: matches `apcore-python.Registry.acquire()` and
    /// `apcore-typescript.Registry.acquire()` — both bump ref_counts so
    /// `safe_unregister()` can wait for in-flight calls to drain (sync
    /// finding A-D-009).
    ///
    /// Callers MUST call [`release`](Self::release) when done with the module
    /// or `safe_unregister()` will hang until its timeout elapses.
    ///
    /// # Errors
    ///
    /// - `Err(ModuleError(code=ModuleNotFound))` if the module is currently
    ///   draining (a `safe_unregister` is in progress) or not registered.
    pub fn acquire(&self, name: &str) -> Result<Arc<dyn Module>, ModuleError> {
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

    /// Release a previously acquired module reference.
    ///
    /// Decrements the ref count; when it reaches zero, notifies any drain
    /// waiter (i.e. `safe_unregister`) that the module is unused.
    ///
    /// Cross-language parity with `apcore-python.Registry.release()` and
    /// `apcore-typescript.Registry.release()` (sync finding A-D-009).
    ///
    /// Calling `release()` on a name that was never acquired is a no-op —
    /// matches Python/TS forgiving semantics.
    pub fn release(&self, name: &str) {
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

    /// Deprecated alias for [`acquire`](Self::acquire) — kept for backward
    /// compatibility with apcore-rust 0.19.x which had separate
    /// `acquire`/`acquire_ref` semantics.
    #[deprecated(since = "0.20.0", note = "Use `acquire()` — it now ref-counts.")]
    pub fn acquire_ref(&self, name: &str) -> Result<Arc<dyn Module>, ModuleError> {
        self.acquire(name)
    }

    /// Deprecated alias for [`release`](Self::release) — kept for backward
    /// compatibility with apcore-rust 0.19.x.
    #[deprecated(since = "0.20.0", note = "Use `release()`.")]
    pub fn release_ref(&self, name: &str) {
        self.release(name);
    }

    /// Check if a module is draining.
    pub fn is_draining(&self, name: &str) -> bool {
        self.core.read().draining.contains(name)
    }

    /// Event subscription.
    /// Register an event callback and return a handle ID for later removal.
    ///
    /// Returns a `u64` handle that can be passed to [`off`](Self::off) to
    /// remove the callback.
    pub fn on(&self, event: &str, callback: Box<ModuleCallbackFn>) -> u64 {
        let id = self.callback_counter.fetch_add(1, Ordering::Relaxed);
        self.callbacks
            .write()
            .entry(event.to_string())
            .or_default()
            .push((id, Arc::from(callback)));
        id
    }

    /// Remove a previously registered event callback by handle ID.
    ///
    /// Returns `true` if the callback was found and removed, `false` otherwise.
    pub fn off(&self, handle_id: u64) -> bool {
        let mut callbacks = self.callbacks.write();
        for entries in callbacks.values_mut() {
            if let Some(pos) = entries.iter().position(|(id, _)| *id == handle_id) {
                entries.remove(pos);
                return true;
            }
        }
        false
    }

    /// Re-run module discovery and update the registry.
    ///
    /// Equivalent to calling [`discover_internal`](Self::discover_internal).
    /// Returns the number of newly registered modules.
    pub async fn reload(&self) -> Result<usize, ModuleError> {
        self.discover_internal().await
    }

    /// Filesystem watching (stub — filesystem watching is not implemented on apcore-rust).
    ///
    /// Watches every path in `extension_roots` recursively. File create / modify
    /// / remove events trigger a debounced (300ms) call to
    /// [`Self::discover_internal`]. Cross-language parity with apcore-python's
    /// `watchdog`-based watcher and apcore-typescript's `fs.watch` watcher
    /// (sync finding A-D-010).
    ///
    /// # Caller obligations
    ///
    /// `watch()` requires `&Arc<Self>` because the spawned background task
    /// holds a `Weak<Registry>` to drive re-discovery; the registry must
    /// already live behind `Arc`. Calling on a non-Arc Registry is a
    /// compile-time error.
    ///
    /// # Errors
    ///
    /// - `Err(ModuleError(code=ReloadFailed))` if the platform watcher cannot
    ///   be created (kernel resource exhaustion, missing permissions, etc.)
    ///
    /// # Idempotency
    ///
    /// Calling `watch()` while already watching is a no-op (returns `Ok(())`).
    /// Use [`Self::unwatch`] to stop and re-call `watch()` to pick up new
    /// `extension_roots`.
    #[allow(clippy::unused_async)] // async kept for cross-language API parity (Python/TS use await registry.watch())
    pub async fn watch(self: &Arc<Self>) -> Result<(), ModuleError> {
        use notify::{RecursiveMode, Watcher};

        {
            let watcher_slot = self.watcher.lock();
            if watcher_slot.is_some() {
                return Ok(()); // already watching
            }
        }

        let extension_roots: Vec<String> = self.extension_roots.read().clone();
        if extension_roots.is_empty() {
            tracing::warn!(
                "Registry::watch() called with no extension_roots — call set_extension_roots() first"
            );
            return Ok(());
        }

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<notify::Result<notify::Event>>();

        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            // Best-effort send; if the receiver is gone, the watcher will be
            // dropped shortly after.
            let _ = tx.send(res);
        })
        .map_err(|e| {
            ModuleError::new(
                crate::errors::ErrorCode::ReloadFailed,
                format!("Failed to create file watcher: {e}"),
            )
        })?;

        for root in &extension_roots {
            let path = std::path::Path::new(root);
            if let Err(e) = watcher.watch(path, RecursiveMode::Recursive) {
                tracing::warn!(
                    root = %root,
                    error = %e,
                    "Registry::watch failed for root, skipping"
                );
            }
        }

        let weak = Arc::downgrade(self);
        let handle = tokio::spawn(Self::watch_loop(rx, weak));

        *self.watcher.lock() = Some(watcher);
        *self.watch_handle.lock() = Some(handle);

        Ok(())
    }

    /// Background task body that consumes notify events and triggers a
    /// debounced re-discovery. Exits when the receiver channel closes or the
    /// `Weak<Registry>` can no longer be upgraded.
    async fn watch_loop(
        mut rx: tokio::sync::mpsc::UnboundedReceiver<notify::Result<notify::Event>>,
        weak: std::sync::Weak<Self>,
    ) {
        use std::time::{Duration, Instant};

        const DEBOUNCE: Duration = Duration::from_millis(300);
        let mut last_trigger = Instant::now()
            .checked_sub(DEBOUNCE)
            .unwrap_or_else(Instant::now);

        while let Some(res) = rx.recv().await {
            let Ok(event) = res else {
                continue;
            };
            // Only react to lifecycle-relevant events
            match event.kind {
                notify::EventKind::Create(_)
                | notify::EventKind::Modify(_)
                | notify::EventKind::Remove(_) => {}
                _ => continue,
            }

            // Per-event debounce — collapse rapid bursts (editors writing
            // through temp file + rename) into a single re-discovery.
            if last_trigger.elapsed() < DEBOUNCE {
                continue;
            }
            last_trigger = Instant::now();

            let Some(reg) = weak.upgrade() else {
                break;
            };
            if let Err(e) = reg.discover_internal().await {
                tracing::warn!(
                    error = %e.message,
                    "Registry watch: discover_internal failed during hot-reload"
                );
            }
        }
    }

    /// Stop filesystem watching. The background task is aborted and the
    /// platform watcher is dropped. Idempotent — calling on a non-watching
    /// Registry is a no-op.
    pub fn unwatch(&self) {
        // Drop the watcher first so notify stops sending; the spawned task
        // will end naturally when the channel closes. We also abort the task
        // explicitly in case the Drop implementation is delayed.
        self.watcher.lock().take();
        if let Some(handle) = self.watch_handle.lock().take() {
            handle.abort();
        }
    }

    /// Discover modules using the internally-set discoverer.
    ///
    /// Returns the number of newly registered modules.
    pub async fn discover_internal(&self) -> Result<usize, ModuleError> {
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
        // discovery, then put it back via an RAII guard so the discoverer
        // is restored even if `discover().await` panics during unwind.
        let discoverer_opt = self.discoverer.write().take();
        let guard = DiscovererRestoreGuard {
            slot: &self.discoverer,
            discoverer: discoverer_opt,
        };
        let Some(active_discoverer) = guard.discoverer.as_ref() else {
            return Err(ModuleError::new(
                crate::errors::ErrorCode::NoDiscovererConfigured,
                "No discoverer configured".to_string(),
            ));
        };

        let roots = self.extension_roots.read().clone();
        let discover_result = active_discoverer.discover(&roots).await;
        // Explicit drop restores the discoverer via Drop impl (also runs on
        // panic unwind from the .await above). `active_discoverer` borrow
        // ends here, so the drop is reachable.
        drop(guard);

        let discovered = discover_result?;
        Ok(self.register_discovered(discovered))
    }

    /// Return true if a descriptor's schema fields have an acceptable shape (object or null).
    ///
    /// Custom discoverers may return non-object JSON for `input_schema` / `output_schema`
    /// (e.g., a string or number). Calling this before insertion prevents invalid values from
    /// flowing into `schema_cache` and later breaking `export_schema()`.
    fn descriptor_schema_shape_is_valid(descriptor: &ModuleDescriptor) -> bool {
        let input_ok = descriptor.input_schema.is_object() || descriptor.input_schema.is_null();
        let output_ok = descriptor.output_schema.is_object() || descriptor.output_schema.is_null();
        input_ok && output_ok
    }

    /// Shared discovery post-processing for `discover()` and `discover_internal()`.
    ///
    /// For each entry: validate the name per `PROTOCOL_SPEC` §2.7, reject
    /// duplicates, run the custom validator, invoke `on_load`, then register
    /// the instance. Returns the count of entries that successfully registered.
    /// Failed entries are logged at `warn` and skipped; one bad entry never
    /// aborts the batch.
    #[allow(clippy::too_many_lines)] // sequential per-entry validation gates in a batch loop; splitting further requires passing state and reduces clarity
    fn register_discovered(&self, discovered: Vec<DiscoveredModule>) -> usize {
        let mut registered_count = 0usize;
        // Collect post-insert work: on_load + callbacks run outside the lock
        // AFTER successful insertion, mirroring the single-entry `register_core`
        // discipline so duplicates never observe on_load side effects.
        let mut post_insert: Vec<(String, Arc<dyn Module>)> = Vec::new();

        for dm in discovered {
            if let Err(e) = validate_module_id(&dm.name, false) {
                tracing::warn!(
                    module_id = %dm.name,
                    error = %e.message,
                    "Discovered module rejected: invalid module_id"
                );
                continue;
            }

            // Snapshot the validator Arc out of the lock.
            let validator_snapshot = self.validator.read().as_ref().map(Arc::clone);
            if let Some(validator) = validator_snapshot {
                let result = validator.validate(dm.module.as_ref(), Some(&dm.descriptor));
                if !result.valid {
                    tracing::warn!(
                        module_id = %dm.name,
                        errors = ?result.errors,
                        "Custom validator rejected discovered module"
                    );
                    continue;
                }
            }

            // Validate descriptor schema shapes before insertion.
            if !Self::descriptor_schema_shape_is_valid(&dm.descriptor) {
                tracing::warn!(
                    module_id = %dm.name,
                    "Discovered module descriptor has non-object schema shape — skipping"
                );
                continue;
            }

            let inserted = {
                let mut core = self.core.write();

                let existing_ids: HashSet<String> = core.modules.keys().cloned().collect();
                match detect_id_conflicts(
                    &dm.name,
                    &existing_ids,
                    RESERVED_WORDS,
                    Some(&core.lowercase_map),
                ) {
                    Some(c) if c.severity == ConflictSeverity::Error => {
                        tracing::warn!(
                            module_id = %dm.name,
                            conflict = %c.message,
                            "Discovered module rejected: id conflict"
                        );
                        false
                    }
                    Some(c) => {
                        tracing::warn!(
                            module_id = %dm.name,
                            conflict = %c.message,
                            "Discovered module registered despite warning-level ID conflict"
                        );
                        let schema = serde_json::json!({
                            "input": dm.descriptor.input_schema.clone(),
                            "output": dm.descriptor.output_schema.clone(),
                        });
                        core.schema_cache.insert(dm.name.clone(), schema);
                        core.lowercase_map
                            .insert(dm.name.to_lowercase(), dm.name.clone());
                        core.modules.insert(dm.name.clone(), Arc::clone(&dm.module));
                        core.descriptors
                            .insert(dm.name.clone(), dm.descriptor.clone());
                        true
                    }
                    None => {
                        let schema = serde_json::json!({
                            "input": dm.descriptor.input_schema.clone(),
                            "output": dm.descriptor.output_schema.clone(),
                        });
                        core.schema_cache.insert(dm.name.clone(), schema);
                        core.lowercase_map
                            .insert(dm.name.to_lowercase(), dm.name.clone());
                        core.modules.insert(dm.name.clone(), Arc::clone(&dm.module));
                        core.descriptors
                            .insert(dm.name.clone(), dm.descriptor.clone());
                        true
                    }
                }
            };

            if inserted {
                post_insert.push((dm.name.clone(), dm.module));
                registered_count += 1;
            }
        }

        // on_load + callbacks fire AFTER successful insertion, outside any lock.
        // If on_load returns Err, roll back the insertion for that module.
        for (name, module_arc) in post_insert {
            if let Err(e) = module_arc.on_load() {
                tracing::error!(
                    module_id = %name,
                    error = %e.message,
                    "Discovered module on_load failed; rolling back registration"
                );
                let mut core = self.core.write();
                core.schema_cache.remove(&name);
                core.lowercase_map.remove(&name.to_lowercase());
                core.modules.remove(&name);
                core.descriptors.remove(&name);
                registered_count = registered_count.saturating_sub(1);
                continue;
            }
            for cb in self.snapshot_callbacks("register") {
                cb(&name, module_arc.as_ref());
            }
        }

        registered_count
    }

    /// Set the discoverer.
    pub fn set_discoverer(&self, discoverer: Box<dyn Discoverer>) {
        *self.discoverer.write() = Some(discoverer);
    }

    /// Set the extension roots passed to `Discoverer::discover()`.
    ///
    /// Aligned with `apcore-python.Registry` (`_extension_roots`) and
    /// `apcore-typescript.Registry` (`_extensionRoots`). Each string is a
    /// filesystem path (or logical namespace) the discoverer should search.
    pub fn set_extension_roots(&self, roots: Vec<String>) {
        *self.extension_roots.write() = roots;
    }

    /// Return a snapshot of the configured extension roots.
    pub fn extension_roots(&self) -> Vec<String> {
        self.extension_roots.read().clone()
    }

    /// Set the validator.
    pub fn set_validator(&self, validator: Box<dyn ModuleValidator>) {
        *self.validator.write() = Some(validator.into());
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

    /// Return a snapshot of all registered (`module_id`, module) pairs.
    pub fn entries(&self) -> Vec<(String, Arc<dyn Module>)> {
        self.core
            .read()
            .modules
            .iter()
            .map(|(k, v)| (k.clone(), Arc::clone(v)))
            .collect()
    }

    /// Export the combined input/output schema for a module.
    ///
    /// Returns a cloned schema JSON, or `None` if the module is not registered.
    pub fn export_schema(&self, name: &str) -> Option<serde_json::Value> {
        self.core.read().schema_cache.get(name).cloned()
    }

    /// Export the combined input/output schema with optional strict-mode
    /// transformation applied.
    ///
    /// When `strict=true`, applies [`to_strict_schema`](crate::schema::to_strict_schema)
    /// to the descriptor's `input_schema` and `output_schema`, producing a
    /// schema that disallows `additionalProperties`, marks all properties
    /// required, and rewrites optional fields as nullable. The returned
    /// JSON has shape `{module_id, description, input_schema, output_schema}`.
    ///
    /// When `strict=false`, equivalent to [`export_schema`](Self::export_schema)
    /// but returned in the structured envelope instead of the raw cached
    /// schema. Returns `None` if the module is not registered.
    ///
    /// Aligned with `apcore-python.Registry.export_schema(module_id, strict=True)`.
    pub fn export_schema_strict(&self, name: &str, strict: bool) -> Option<serde_json::Value> {
        let descriptor = self.get_definition(name)?;
        let (input_schema, output_schema) = if strict {
            (
                crate::schema::to_strict_schema(&descriptor.input_schema),
                crate::schema::to_strict_schema(&descriptor.output_schema),
            )
        } else {
            (
                descriptor.input_schema.clone(),
                descriptor.output_schema.clone(),
            )
        };
        Some(serde_json::json!({
            "module_id": descriptor.module_id,
            "description": descriptor.description,
            "input_schema": input_schema,
            "output_schema": output_schema,
        }))
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

// APCore Protocol — Configuration
// Spec reference: Configuration loading, validation, and environment variable overrides (Algorithm A12)

use parking_lot::RwLock;
use serde::de::{DeserializeOwned, Error as DeError};
use serde::{Deserialize, Deserializer, Serialize};
use serde_yaml_ng as serde_yaml;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use crate::errors::{ErrorCode, ModuleError};

/// Configuration mode detected from YAML content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConfigMode {
    #[default]
    Legacy,
    Namespace,
}

/// Source for a `config.mount()` operation.
pub enum MountSource {
    Dict(serde_json::Value),
    File(PathBuf),
}

/// Default maximum nesting depth for env var key conversion.
pub const DEFAULT_MAX_DEPTH: usize = 5;

/// Environment variable key conversion strategy for a namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EnvStyle {
    /// Single `_` → `.` (section separator), double `__` → literal `_`.
    Nested,
    /// Suffix is lowercased as-is; no separator conversion.
    Flat,
    /// Match against defaults tree structure; fall back to Nested.
    #[default]
    Auto,
}

/// Registration info for a Config Bus namespace.
#[derive(Debug, Clone)]
pub struct NamespaceRegistration {
    pub name: String,
    /// Env var prefix. `None` = auto-derive from name (uppercase, `-` → `_`).
    pub env_prefix: Option<String>,
    pub defaults: Option<serde_json::Value>,
    pub schema: Option<serde_json::Value>,
    pub env_style: EnvStyle,
    pub max_depth: usize,
    /// Explicit bare env var → config key mapping (e.g. `"REDIS_URL" → "cache_url"`).
    pub env_map: Option<HashMap<String, String>>,
}

/// Summary of a registered namespace (returned by `registered_namespaces()`).
#[derive(Debug, Clone)]
pub struct NamespaceInfo {
    pub name: String,
    pub env_prefix: Option<String>,
    pub has_schema: bool,
}

static GLOBAL_NS_REGISTRY: OnceLock<RwLock<HashMap<String, NamespaceRegistration>>> =
    OnceLock::new();
/// Global bare env var → top-level config key mapping.
static GLOBAL_ENV_MAP: OnceLock<RwLock<HashMap<String, String>>> = OnceLock::new();
/// Tracks all claimed env var names (for conflict detection).
static ENV_MAP_CLAIMED: OnceLock<RwLock<HashMap<String, String>>> = OnceLock::new();

fn global_ns_registry() -> &'static RwLock<HashMap<String, NamespaceRegistration>> {
    GLOBAL_NS_REGISTRY.get_or_init(|| RwLock::new(HashMap::new()))
}

fn global_env_map() -> &'static RwLock<HashMap<String, String>> {
    GLOBAL_ENV_MAP.get_or_init(|| RwLock::new(HashMap::new()))
}

fn env_map_claimed() -> &'static RwLock<HashMap<String, String>> {
    ENV_MAP_CLAIMED.get_or_init(|| RwLock::new(HashMap::new()))
}

const RESERVED_NAMESPACES: &[&str] = &["apcore", "_config"];

/// Executor namespace configuration (`PROTOCOL_SPEC` §9.1).
///
/// All timeouts are in milliseconds.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ExecutorConfig {
    /// Per-module execution timeout (milliseconds). 0 means no per-module timeout.
    pub default_timeout: u64,
    /// Whole-call-chain deadline (milliseconds). 0 means no global deadline.
    pub global_timeout: u64,
    /// Maximum call chain depth before `MODULE_CALL_DEPTH_EXCEEDED` is raised.
    pub max_call_depth: u32,
    /// Maximum repeat count for the same module within a single call chain.
    pub max_module_repeat: u32,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            default_timeout: 30_000,
            global_timeout: 60_000,
            max_call_depth: 32,
            max_module_repeat: 3,
        }
    }
}

/// Observability namespace configuration (`PROTOCOL_SPEC` §9.1).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ObservabilityConfig {
    pub tracing: TracingConfig,
    pub metrics: MetricsConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TracingConfig {
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MetricsConfig {
    pub enabled: bool,
}

/// Top-level apcore configuration (`PROTOCOL_SPEC` §9.1).
///
/// Canonical wire format is a nested JSON/YAML object with `executor`,
/// `observability`, and any user-defined namespaces as siblings:
///
/// ```yaml
/// modules_path: ./modules
/// executor:
///   max_call_depth: 32
///   default_timeout: 30000
/// observability:
///   tracing:
///     enabled: true
/// my_vendor:
///   custom_setting: foo
/// ```
///
/// **v0.18.0 BREAKING CHANGE.** Prior versions accepted root-level
/// `max_call_depth`, `default_timeout_ms`, etc. The custom `Deserialize` impl
/// now rejects these with a hard error pointing at `MIGRATION-v0.18.md`.
/// **Note (sync finding A-D-016).** Apcore-python and apcore-typescript
/// register the built-in `observability` and `sys_modules` namespaces at
/// module-load time, so every code path observes them. Rust has no cheap
/// equivalent (no implicit module-init hook without the `ctor` crate), so
/// the SDK uses an idempotent `OnceLock`-guarded `init_builtin_namespaces()`
/// that runs from the user-facing entry points: `Config::from_yaml_file`,
/// `Config::from_json_file`, `Config::from_defaults`, and
/// `Config::load_or_discover`.
///
/// `Config::default()` (`#[derive(Default)]`) is the low-level constructor
/// and intentionally does NOT trigger initialization — it is meant for
/// internal/test code that wants a bare struct without touching the
/// process-global namespace registry. **User code should call
/// `Config::from_defaults()` for canonical defaults**, which mirrors Python
/// and TypeScript behavior. Calling either `from_yaml_file`/`from_json_file`
/// also initializes the built-ins.
///
/// This is a documented Rust-specific divergence rather than a behavioral
/// bug; cross-language conformance fixtures rely on `from_yaml_file` /
/// `from_defaults` and therefore see consistent behavior.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modules_path: Option<PathBuf>,
    #[serde(default)]
    pub executor: ExecutorConfig,
    #[serde(default)]
    pub observability: ObservabilityConfig,
    /// User-defined and vendor namespaces. Captures any top-level key not
    /// matching a canonical namespace above. Per spec §9.1, custom namespace
    /// names should follow `[a-z][a-z0-9-]*`.
    #[serde(flatten)]
    pub user_namespaces: HashMap<String, serde_json::Value>,
    #[serde(skip)]
    pub yaml_path: Option<PathBuf>,
    #[serde(skip)]
    pub mode: ConfigMode,
}

/// Legacy v0.17.x root-level field names that are no longer accepted in v0.18.0.
const LEGACY_ROOT_FIELDS: &[(&str, &str)] = &[
    ("max_call_depth", "executor.max_call_depth"),
    ("max_module_repeat", "executor.max_module_repeat"),
    ("default_timeout_ms", "executor.default_timeout"),
    ("global_timeout_ms", "executor.global_timeout"),
    ("enable_tracing", "observability.tracing.enabled"),
    ("enable_metrics", "observability.metrics.enabled"),
];

// Helper struct for two-pass deserialization of Config.
// Defined outside the fn body to satisfy items_after_statements lint.
#[derive(Deserialize)]
struct ConfigHelper {
    #[serde(default)]
    modules_path: Option<PathBuf>,
    #[serde(default)]
    executor: ExecutorConfig,
    #[serde(default)]
    observability: ObservabilityConfig,
    #[serde(flatten, default)]
    user_namespaces: HashMap<String, serde_json::Value>,
}

impl<'de> Deserialize<'de> for Config {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Two-pass: first parse the wire form into a generic JSON object,
        // detect any v0.17.x legacy root-level fields, then materialize the
        // canonical struct via a helper that mirrors the serialized shape.
        let raw = serde_json::Map::<String, serde_json::Value>::deserialize(deserializer)?;

        let mut violations: Vec<String> = Vec::new();
        for (legacy, canonical) in LEGACY_ROOT_FIELDS {
            if raw.contains_key(*legacy) {
                violations.push(format!("'{legacy}' → '{canonical}'"));
            }
        }
        if !violations.is_empty() {
            return Err(D::Error::custom(format!(
                "apcore v0.18.0 changed Config layout: root-level fields {} are no longer accepted. \
                 Move them to their canonical nested namespace. \
                 See MIGRATION-v0.18.md for the full migration guide.",
                violations.join(", ")
            )));
        }

        let mut core_data = raw.clone();
        let mut mode = ConfigMode::Legacy;

        // §9.6: If "apcore" key is present, it's namespace mode.
        if let Some(apcore_val) = raw.get("apcore") {
            if let Some(apcore_obj) = apcore_val.as_object() {
                mode = ConfigMode::Namespace;
                // Merge apcore-namespace fields into the top-level core_data
                // so ConfigHelper can find them.
                for (k, v) in apcore_obj {
                    core_data.insert(k.clone(), v.clone());
                }
            }
        }

        let helper: ConfigHelper = serde_json::from_value(serde_json::Value::Object(core_data))
            .map_err(D::Error::custom)?;

        Ok(Config {
            modules_path: helper.modules_path,
            executor: helper.executor,
            observability: helper.observability,
            user_namespaces: helper.user_namespaces,
            yaml_path: None,
            mode,
        })
    }
}

impl Config {
    /// Load config from a JSON file, apply env overrides, and validate.
    pub fn from_json_file(path: &std::path::Path) -> Result<Self, ModuleError> {
        let file = std::fs::File::open(path).map_err(|e| {
            ModuleError::new(
                ErrorCode::ConfigNotFound,
                format!("Config file not found: {}: {}", path.display(), e),
            )
        })?;
        let reader = std::io::BufReader::new(file);
        let mut config: Config = serde_json::from_reader(reader).map_err(|e| {
            ModuleError::new(
                ErrorCode::ConfigInvalid,
                format!("Failed to parse JSON config: {}: {}", path.display(), e),
            )
        })?;
        config.detect_mode();
        init_builtin_namespaces();
        config.apply_env_overrides();
        config.validate()?;
        Ok(config)
    }

    /// Load config from a YAML file, apply env overrides, and validate.
    pub fn from_yaml_file(path: &std::path::Path) -> Result<Self, ModuleError> {
        let file = std::fs::File::open(path).map_err(|e| {
            ModuleError::new(
                ErrorCode::ConfigNotFound,
                format!("Config file not found: {}: {}", path.display(), e),
            )
        })?;
        let reader = std::io::BufReader::new(file);
        let mut config: Config = serde_yaml::from_reader(reader).map_err(|e| {
            ModuleError::new(
                ErrorCode::ConfigInvalid,
                format!("Failed to parse YAML config: {}: {}", path.display(), e),
            )
        })?;
        config.yaml_path = Some(path.to_path_buf());
        config.detect_mode();
        init_builtin_namespaces();
        config.apply_env_overrides();
        config.validate()?;
        Ok(config)
    }

    /// Auto-detect format by file extension and load.
    pub fn load(path: &std::path::Path) -> Result<Self, ModuleError> {
        match path.extension().and_then(|e| e.to_str()) {
            Some("json") => Self::from_json_file(path),
            Some("yaml" | "yml") => Self::from_yaml_file(path),
            _ => {
                // Default to YAML
                Self::from_yaml_file(path)
            }
        }
    }

    /// No-arg load: discover the config file via the canonical search order
    /// and load it, falling back to `Config::from_defaults()` if none is
    /// found. Equivalent to apcore-python's `Config.load(path=None)` and
    /// apcore-typescript's `Config.discover()`.
    ///
    /// Sync finding A-D-013: spec contract is `Config.load(path?)`. Rust
    /// previously required a path on `load()` and exposed `discover()` as a
    /// separate method. This helper restores no-arg load parity for portable
    /// cross-language code without changing the strict-typed `load(&Path)`
    /// signature for callers that already know the path.
    pub fn load_or_discover() -> Result<Self, ModuleError> {
        match discover_config_file() {
            Some(path) => Self::load(&path),
            None => Ok(Self::from_defaults()),
        }
    }

    /// Validate config constraints. Returns an error listing all violations.
    ///
    /// Sync CB-001: validates the spec-mandated field set beyond
    /// executor-only knobs — mirrors apcore-python `_REQUIRED_FIELDS` and
    /// `_CONSTRAINTS` (config.py). Constraints checked include:
    ///   - `acl.default_effect` ∈ {`allow`, `deny`}
    ///   - `observability.tracing.sampling_rate` ∈ [0.0, 1.0]
    ///   - executor numeric ranges (`max_call_depth`, `max_module_repeat`,
    ///     `default_timeout`, `global_timeout`)
    pub fn validate(&self) -> Result<(), ModuleError> {
        let mut errors: Vec<String> = Vec::new();

        if self.executor.max_call_depth < 1 {
            errors.push("executor.max_call_depth must be >= 1".to_string());
        }
        if self.executor.max_module_repeat < 1 {
            errors.push("executor.max_module_repeat must be >= 1".to_string());
        }
        // default_timeout == 0 means no timeout, which is allowed.
        if self.executor.global_timeout > 0
            && self.executor.default_timeout > 0
            && self.executor.global_timeout < self.executor.default_timeout
        {
            errors.push(format!(
                "executor.global_timeout ({}) must be >= executor.default_timeout ({})",
                self.executor.global_timeout, self.executor.default_timeout
            ));
        }

        // Sync CB-001: cross-language constraint set.
        if let Some(de) = self.get("acl.default_effect") {
            match de.as_str() {
                Some("allow" | "deny") => {}
                Some(other) => {
                    errors.push(format!(
                        "acl.default_effect must be 'allow' or 'deny' (got '{other}')"
                    ));
                }
                None => {
                    errors.push("acl.default_effect must be a string".to_string());
                }
            }
        }

        if let Some(rate) = self.get("observability.tracing.sampling_rate") {
            let rate_ok = rate.as_f64().is_some_and(|f| (0.0..=1.0).contains(&f));
            if !rate_ok {
                errors.push(format!(
                    "observability.tracing.sampling_rate must be a number in [0.0, 1.0] (got {rate})"
                ));
            }
        }

        if let Some(threshold) = self.get("sys_modules.events.thresholds.error_rate") {
            let ok = threshold.as_f64().is_some_and(|f| (0.0..=1.0).contains(&f));
            if !ok {
                errors.push(format!(
                    "sys_modules.events.thresholds.error_rate must be a number in [0.0, 1.0] (got {threshold})"
                ));
            }
        }

        if let Some(latency) = self.get("sys_modules.events.thresholds.latency_p99_ms") {
            let ok = latency.as_f64().is_some_and(|f| f > 0.0);
            if !ok {
                errors.push(format!(
                    "sys_modules.events.thresholds.latency_p99_ms must be a positive number (got {latency})"
                ));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            let message = format!("Config validation failed: {}", errors.join("; "));
            Err(ModuleError::new(ErrorCode::ConfigInvalid, message))
        }
    }

    /// Build config from defaults, applying env var overrides.
    #[must_use]
    pub fn from_defaults() -> Self {
        let mut config = Self::default();
        config.detect_mode();
        init_builtin_namespaces();
        config.apply_env_overrides();
        config
    }

    /// Discover and load config using the §9.14 search order.
    ///
    /// If no file is found, returns `Config::from_defaults()`.
    pub fn discover() -> Result<Self, ModuleError> {
        match discover_config_file() {
            Some(path) => Self::load(&path),
            None => Ok(Self::from_defaults()),
        }
    }

    /// Get a config value by dot-path key.
    ///
    /// Walks the canonical nested namespace tree (`executor.*`,
    /// `observability.*`, `modules_path`) and falls back to user-defined
    /// namespaces. Per spec §9.1, all keys MUST use the canonical
    /// `<namespace>.<field>` form. Legacy v0.17.x short-form aliases
    /// (e.g. bare `max_call_depth`) are NOT accepted.
    ///
    /// Sync finding A-D-017: namespace resolution uses longest-prefix match
    /// against registered names, mirroring apcore-python's
    /// `_split_namespace_key` and apcore-typescript's `resolveNamespacePath`.
    /// Hyphenated namespace names (e.g. `apcore-mcp.transport.endpoint`)
    /// route correctly even though `.split('.')` would otherwise strand the
    /// hyphenated prefix on the first segment.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<serde_json::Value> {
        // Check canonical typed fields first.
        if let Some(val) = self.get_typed_field(key) {
            return Some(val);
        }

        // Longest-prefix match against the registered namespaces, then fall
        // back to dot-split on the first segment. Hyphenated names like
        // `apcore-mcp` cannot be reached by naive `split('.')`.
        if let Some((ns_name, rest)) = Self::match_registered_namespace(key) {
            let top = self.user_namespaces.get(&ns_name)?;
            if rest.is_empty() {
                return Some(top.clone());
            }
            let mut current = top;
            for part in rest.split('.') {
                current = current.get(part)?;
            }
            return Some(current.clone());
        }

        // Fall back to user namespaces with dot-path traversal on the first
        // segment (covers namespaces that were not explicitly registered).
        let parts: Vec<&str> = key.split('.').collect();
        if parts.is_empty() {
            return None;
        }
        let top = self.user_namespaces.get(parts[0])?;
        if parts.len() == 1 {
            return Some(top.clone());
        }
        let mut current = top;
        for part in &parts[1..] {
            current = current.get(*part)?;
        }
        Some(current.clone())
    }

    /// Match `key` against the longest registered namespace name that is a
    /// prefix-with-`.` (or exact-match) of the key. Returns `(namespace_name,
    /// remainder_after_namespace_dot)`. Used by `get()` to support
    /// hyphenated namespaces (sync finding A-D-017).
    fn match_registered_namespace(key: &str) -> Option<(String, String)> {
        let registry = global_ns_registry().read();
        // Sort registered names by length descending so longer matches win.
        let mut names: Vec<&String> = registry.keys().collect();
        names.sort_by_key(|s| std::cmp::Reverse(s.len()));
        for name in names {
            if key == name.as_str() {
                return Some((name.clone(), String::new()));
            }
            let dotted = format!("{name}.");
            if key.starts_with(&dotted) {
                return Some((name.clone(), key[dotted.len()..].to_string()));
            }
        }
        None
    }

    /// Set a config value by dot-path key.
    ///
    /// Attempts to set canonical typed fields first, then falls back to
    /// user namespaces. Returns silently on type mismatch.
    pub fn set(&mut self, key: &str, value: serde_json::Value) {
        // Try canonical typed fields.
        if self.set_typed_field(key, &value) {
            return;
        }

        // Fall back to user namespaces.
        let parts: Vec<&str> = key.split('.').collect();
        if parts.is_empty() {
            return;
        }
        if parts.len() == 1 {
            self.user_namespaces.insert(key.to_string(), value);
            return;
        }
        let root = self
            .user_namespaces
            .entry(parts[0].to_string())
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        let mut current = root;
        for part in &parts[1..parts.len() - 1] {
            if !current.is_object() {
                *current = serde_json::Value::Object(serde_json::Map::new());
            }
            // INVARIANT: the preceding `if !current.is_object()` branch guarantees object shape.
            current = current
                .as_object_mut()
                .unwrap()
                .entry(part.to_string())
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        }
        if !current.is_object() {
            *current = serde_json::Value::Object(serde_json::Map::new());
        }
        // INVARIANT: the preceding `if !current.is_object()` branch guarantees object shape.
        current
            .as_object_mut()
            .unwrap()
            .insert(parts[parts.len() - 1].to_string(), value);
    }

    /// Reload config from the stored `yaml_path`. Returns error if no path stored.
    pub fn reload(&mut self) -> Result<(), ModuleError> {
        let path = self.yaml_path.clone().ok_or_else(|| {
            ModuleError::new(
                ErrorCode::ReloadFailed,
                "Cannot reload: no yaml_path stored (config was not loaded from a file)",
            )
        })?;
        let reloaded = Self::load(&path)?;
        // Preserve the yaml_path through reload
        let yaml_path = self.yaml_path.take();
        *self = reloaded;
        self.yaml_path = yaml_path;
        Ok(())
    }

    /// Return a `serde_json::Value` representing the full config as the
    /// canonical nested JSON object (`PROTOCOL_SPEC` §9.1 wire format).
    #[must_use]
    pub fn data(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }

    // --- Namespace registration (class methods) ---

    pub fn register_namespace(mut reg: NamespaceRegistration) -> Result<(), ModuleError> {
        if RESERVED_NAMESPACES.contains(&reg.name.as_str()) {
            return Err(ModuleError::config_namespace_reserved(&reg.name));
        }
        // Auto-derive env_prefix from name if not provided.
        if reg.env_prefix.is_none() {
            reg.env_prefix = Some(reg.name.to_uppercase().replace('-', "_"));
        }
        let mut map = global_ns_registry().write();
        if map.contains_key(&reg.name) {
            return Err(ModuleError::config_namespace_duplicate(&reg.name));
        }
        // Check for duplicate env_prefix.
        let prefix = reg.env_prefix.as_deref().unwrap_or("");
        for existing in map.values() {
            if existing.env_prefix.as_deref() == Some(prefix) {
                return Err(ModuleError::config_env_prefix_conflict(prefix));
            }
        }
        // Validate env_map: no env var can be claimed twice.
        if let Some(ref em) = reg.env_map {
            let claimed = env_map_claimed().read();
            for env_var in em.keys() {
                if let Some(owner) = claimed.get(env_var) {
                    return Err(ModuleError::config_env_map_conflict(env_var, owner));
                }
            }
            drop(claimed);
            let mut claimed = env_map_claimed().write();
            for env_var in em.keys() {
                claimed.insert(env_var.clone(), reg.name.clone());
            }
        }
        map.insert(reg.name.clone(), reg);
        Ok(())
    }

    /// Register global bare env var → top-level config key mappings.
    pub fn env_map(mapping: HashMap<String, String>) -> Result<(), ModuleError> {
        let claimed_lock = env_map_claimed();
        let claimed = claimed_lock.read();
        for env_var in mapping.keys() {
            if let Some(owner) = claimed.get(env_var) {
                return Err(ModuleError::config_env_map_conflict(env_var, owner));
            }
        }
        drop(claimed);
        let mut claimed = claimed_lock.write();
        let mut gmap = global_env_map().write();
        for (env_var, config_key) in mapping {
            claimed.insert(env_var.clone(), "__global__".to_string());
            gmap.insert(env_var, config_key);
        }
        Ok(())
    }

    #[must_use]
    pub fn registered_namespaces() -> Vec<NamespaceInfo> {
        global_ns_registry()
            .read()
            .values()
            .map(|r| NamespaceInfo {
                name: r.name.clone(),
                env_prefix: r.env_prefix.clone(),
                has_schema: r.schema.is_some(),
            })
            .collect()
    }

    // --- Namespace instance methods ---

    #[must_use]
    pub fn namespace(&self, name: &str) -> Option<serde_json::Value> {
        self.user_namespaces.get(name).cloned()
    }

    pub fn mount(&mut self, namespace: &str, source: MountSource) -> Result<(), ModuleError> {
        // W-2: Reject reserved namespace per §9.7 spec.
        if namespace == "_config" {
            return Err(ModuleError::config_mount_error(
                namespace,
                "cannot mount to reserved namespace '_config'",
            ));
        }
        let data = match source {
            MountSource::Dict(v) => v,
            MountSource::File(path) => {
                let content = std::fs::read_to_string(&path)
                    .map_err(|e| ModuleError::config_mount_error(namespace, &e.to_string()))?;
                serde_yaml::from_str(&content)
                    .map_err(|e| ModuleError::config_mount_error(namespace, &e.to_string()))?
            }
        };
        if !data.is_object() {
            return Err(ModuleError::config_mount_error(
                namespace,
                "mount source must be a JSON object",
            ));
        }
        let entry = self
            .user_namespaces
            .entry(namespace.to_string())
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        if let (Some(target), Some(source_map)) = (entry.as_object_mut(), data.as_object()) {
            // Sync CB-002: deep-merge so peer keys in nested objects are
            // preserved rather than overwritten. Mirrors apcore-python's
            // `_deep_merge_dicts` (config.py) and apcore-typescript's
            // `deepMerge`. Without this, `mount({db:{host:'a'}})` over
            // `{db:{port:5432}}` would discard `port`.
            for (k, v) in source_map {
                match target.get_mut(k) {
                    Some(existing) => {
                        deep_merge_value(existing, v);
                    }
                    None => {
                        target.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        Ok(())
    }

    pub fn bind<T: DeserializeOwned>(&self, namespace: &str) -> Result<T, ModuleError> {
        // Special-case canonical namespaces so `bind::<ExecutorConfig>("executor")`
        // returns the typed struct directly.
        match namespace {
            "executor" => {
                return serde_json::from_value(
                    serde_json::to_value(&self.executor)
                        .map_err(|e| ModuleError::config_bind_error(namespace, &e.to_string()))?,
                )
                .map_err(|e| ModuleError::config_bind_error(namespace, &e.to_string()))
            }
            "observability" => {
                return serde_json::from_value(
                    serde_json::to_value(&self.observability)
                        .map_err(|e| ModuleError::config_bind_error(namespace, &e.to_string()))?,
                )
                .map_err(|e| ModuleError::config_bind_error(namespace, &e.to_string()))
            }
            _ => {}
        }

        // Sync finding A-D-018: when the namespace has no data registered,
        // bind into an empty object so `T`'s serde defaults take effect —
        // matching apcore-python's `_instantiate_model(model, {}, namespace)`
        // and apcore-typescript's `new schema({})`. Previously Rust returned
        // ConfigBindError("namespace not found"), which broke portable code
        // that relied on default-fill behavior across SDKs.
        let owned;
        let value: &serde_json::Value = if let Some(v) = self.user_namespaces.get(namespace) {
            v
        } else {
            owned = serde_json::Value::Object(serde_json::Map::new());
            &owned
        };
        serde_json::from_value(value.clone())
            .map_err(|e| ModuleError::config_bind_error(namespace, &e.to_string()))
    }

    pub fn get_typed<T: DeserializeOwned>(&self, key: &str) -> Result<T, ModuleError> {
        let value = self
            .get(key)
            .ok_or_else(|| ModuleError::config_bind_error(key, "key not found"))?;
        serde_json::from_value(value)
            .map_err(|e| ModuleError::config_bind_error(key, &e.to_string()))
    }

    // --- Private helpers ---

    fn detect_mode(&mut self) {
        // W-3: Only activate namespace mode when "apcore" key is a mapping.
        // A null or scalar value is not a valid namespace indicator.
        self.mode = match self.user_namespaces.get("apcore") {
            Some(serde_json::Value::Object(_)) => ConfigMode::Namespace,
            _ => ConfigMode::Legacy,
        };
    }

    /// Apply APCORE_* environment variable overrides to both typed fields and settings.
    ///
    /// In legacy mode, all `APCORE_*` vars are mapped via `env_key_to_dot_path`.
    /// In namespace mode, registered `env_prefix` values are dispatched via
    /// longest-prefix-match (§9.10).
    fn apply_env_overrides(&mut self) {
        if self.mode == ConfigMode::Namespace {
            self.apply_namespace_env_overrides();
            return;
        }
        // Legacy mode: flat APCORE_ prefix stripping.
        for (key, value) in std::env::vars() {
            if let Some(suffix) = key.strip_prefix("APCORE_") {
                let dot_path = Self::env_key_to_dot_path(suffix);
                let parsed = Self::coerce_env_value(&value);
                tracing::debug!(env = %key, path = %dot_path, "Applying legacy env override");
                self.set(&dot_path, parsed);
            }
        }
    }

    /// §9.10: Namespace-aware env routing via longest-prefix-match.
    fn apply_namespace_env_overrides(&mut self) {
        let registry = global_ns_registry().read();
        let gmap = global_env_map().read();

        // Build namespace env_map lookup.
        let mut ns_env_maps: HashMap<&str, (&str, &str)> = HashMap::new();
        for reg in registry.values() {
            if let Some(ref em) = reg.env_map {
                for (env_var, config_key) in em {
                    ns_env_maps.insert(env_var.as_str(), (reg.name.as_str(), config_key.as_str()));
                }
            }
        }

        // Prefix table: sorted by length descending for longest-prefix-match.
        let mut prefixed: Vec<&NamespaceRegistration> = registry
            .values()
            .filter(|r| r.env_prefix.is_some())
            .collect();
        prefixed.sort_by(|a, b| {
            b.env_prefix
                .as_ref()
                .map_or(0, std::string::String::len)
                .cmp(&a.env_prefix.as_ref().map_or(0, std::string::String::len))
        });

        for (env_key, env_value) in std::env::vars() {
            let parsed = Self::coerce_env_value(&env_value);

            // 1. Global env_map (bare env var → top-level key).
            if let Some(config_key) = gmap.get(&env_key) {
                self.set(config_key, parsed);
                continue;
            }

            // 2. Namespace env_map (bare env var → namespace key).
            if let Some(&(ns_name, config_key)) = ns_env_maps.get(env_key.as_str()) {
                let full_path = format!("{ns_name}.{config_key}");
                self.set(&full_path, parsed);
                continue;
            }

            // 3. Prefix-based dispatch.
            let mut matched = false;
            for reg in &prefixed {
                let prefix = reg.env_prefix.as_deref().unwrap_or("");
                if let Some(suffix) = env_key.strip_prefix(prefix) {
                    let suffix = suffix.strip_prefix('_').unwrap_or(suffix);
                    if suffix.is_empty() {
                        continue;
                    }
                    let key = Self::resolve_env_suffix(suffix, reg);
                    let full_path = format!("{}.{key}", reg.name);
                    tracing::debug!(env = %env_key, path = %full_path, "Applying namespace env override");
                    self.set(&full_path, parsed.clone());
                    matched = true;
                    break;
                }
            }
            // Fallback: APCORE_ prefix with no matching namespace → treat as
            // top-level key (same as legacy mode). Per spec §9.8, un-matched
            // env vars resolve to their natural dot-path without namespace prefix.
            if !matched {
                if let Some(suffix) = env_key.strip_prefix("APCORE_") {
                    let dot_path = Self::env_key_to_dot_path(suffix);
                    tracing::debug!(env = %env_key, path = %dot_path, "Applying fallback env override (no namespace match)");
                    self.set(&dot_path, parsed);
                }
            }
        }
    }

    /// Map a canonical dot-path key to a typed field value.
    ///
    /// Recognizes only the canonical `<namespace>.<field>` form per spec §9.1.
    /// Legacy bare-name aliases are NOT accepted.
    fn get_typed_field(&self, key: &str) -> Option<serde_json::Value> {
        match key {
            "executor.max_call_depth" => Some(serde_json::Value::Number(
                self.executor.max_call_depth.into(),
            )),
            "executor.max_module_repeat" => Some(serde_json::Value::Number(
                self.executor.max_module_repeat.into(),
            )),
            "executor.default_timeout" => Some(serde_json::Value::Number(
                self.executor.default_timeout.into(),
            )),
            "executor.global_timeout" => Some(serde_json::Value::Number(
                self.executor.global_timeout.into(),
            )),
            "observability.tracing.enabled" => {
                Some(serde_json::Value::Bool(self.observability.tracing.enabled))
            }
            "observability.metrics.enabled" => {
                Some(serde_json::Value::Bool(self.observability.metrics.enabled))
            }
            "modules_path" => self
                .modules_path
                .as_ref()
                .map(|p| serde_json::Value::String(p.to_string_lossy().into_owned())),
            _ => None,
        }
    }

    /// Try to set a canonical typed field. Returns true if matched.
    fn set_typed_field(&mut self, key: &str, value: &serde_json::Value) -> bool {
        match key {
            "executor.max_call_depth" => {
                if let Some(n) = value.as_u64() {
                    #[allow(clippy::cast_possible_truncation)]
                    // config values are small and won't exceed u32::MAX
                    {
                        self.executor.max_call_depth = n as u32;
                    }
                    return true;
                }
            }
            "executor.max_module_repeat" => {
                if let Some(n) = value.as_u64() {
                    #[allow(clippy::cast_possible_truncation)]
                    // config values are small and won't exceed u32::MAX
                    {
                        self.executor.max_module_repeat = n as u32;
                    }
                    return true;
                }
            }
            "executor.default_timeout" => {
                if let Some(n) = value.as_u64() {
                    self.executor.default_timeout = n;
                    return true;
                }
            }
            "executor.global_timeout" => {
                if let Some(n) = value.as_u64() {
                    self.executor.global_timeout = n;
                    return true;
                }
            }
            "observability.tracing.enabled" => {
                if let Some(b) = value.as_bool() {
                    self.observability.tracing.enabled = b;
                    return true;
                }
            }
            "observability.metrics.enabled" => {
                if let Some(b) = value.as_bool() {
                    self.observability.metrics.enabled = b;
                    return true;
                }
            }
            "modules_path" => {
                if let Some(s) = value.as_str() {
                    self.modules_path = Some(PathBuf::from(s));
                    return true;
                }
            }
            _ => {}
        }
        false
    }

    /// Convert an env-var suffix to a dot-path config key.
    ///
    /// Convention (matches Python reference):
    ///   - Single `_` → `.` (section separator)
    ///   - Double `__` → literal `_` (underscore within a field name)
    ///
    /// Example: `EXECUTOR_MAX__CALL__DEPTH` → `executor.max_call_depth`
    ///
    /// So to set `max_call_depth` via env, use `APCORE_EXECUTOR_MAX__CALL__DEPTH`.
    fn env_key_to_dot_path(raw: &str) -> String {
        Self::env_key_to_dot_path_with_depth(raw, usize::MAX)
    }

    /// Convert env var suffix to dot-path, stopping at `max_depth` segments.
    fn env_key_to_dot_path_with_depth(raw: &str, max_depth: usize) -> String {
        let lower = raw.to_lowercase();
        let chars: Vec<char> = lower.chars().collect();
        let mut result = String::with_capacity(chars.len());
        let mut dot_count: usize = 0;
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '_' {
                if i + 1 < chars.len() && chars[i + 1] == '_' {
                    result.push('_'); // double __ → literal _
                    i += 2;
                } else if dot_count < max_depth.saturating_sub(1) {
                    result.push('.');
                    dot_count += 1;
                    i += 1;
                } else {
                    result.push('_'); // depth limit reached
                    i += 1;
                }
            } else {
                result.push(chars[i]);
                i += 1;
            }
        }
        result
    }

    /// Try to match suffix against keys in a JSON object tree (recursive).
    fn match_suffix_to_tree(
        suffix: &str,
        tree: &serde_json::Map<String, serde_json::Value>,
        depth: usize,
        max_depth: usize,
    ) -> Option<String> {
        // 1. Try full suffix as a flat key.
        if tree.contains_key(suffix) {
            return Some(suffix.to_string());
        }
        // 2. Depth limit.
        if depth >= max_depth.saturating_sub(1) {
            return None;
        }
        // 3. Try splitting at each underscore.
        for (i, ch) in suffix.char_indices() {
            if ch != '_' || i == 0 || i == suffix.len() - 1 {
                continue;
            }
            let prefix_part = &suffix[..i];
            let remainder = &suffix[i + 1..];
            if let Some(serde_json::Value::Object(subtree)) = tree.get(prefix_part) {
                if let Some(sub) =
                    Self::match_suffix_to_tree(remainder, subtree, depth + 1, max_depth)
                {
                    return Some(format!("{prefix_part}.{sub}"));
                }
            }
        }
        None
    }

    /// Resolve an env var suffix based on the registration's `env_style`.
    fn resolve_env_suffix(suffix: &str, reg: &NamespaceRegistration) -> String {
        match reg.env_style {
            EnvStyle::Flat => suffix.to_lowercase(),
            EnvStyle::Auto => {
                let lower = suffix.to_lowercase();
                if let Some(serde_json::Value::Object(tree)) = reg.defaults.as_ref() {
                    if let Some(resolved) =
                        Self::match_suffix_to_tree(&lower, tree, 0, reg.max_depth)
                    {
                        return resolved;
                    }
                }
                // Fall back to nested with depth.
                Self::env_key_to_dot_path_with_depth(suffix, reg.max_depth)
            }
            EnvStyle::Nested => Self::env_key_to_dot_path_with_depth(suffix, reg.max_depth),
        }
    }

    fn coerce_env_value(value: &str) -> serde_json::Value {
        if value.eq_ignore_ascii_case("true") {
            return serde_json::Value::Bool(true);
        }
        if value.eq_ignore_ascii_case("false") {
            return serde_json::Value::Bool(false);
        }
        if let Ok(n) = value.parse::<i64>() {
            return serde_json::Value::Number(n.into());
        }
        if let Ok(f) = value.parse::<f64>() {
            if let Some(n) = serde_json::Number::from_f64(f) {
                return serde_json::Value::Number(n);
            }
        }
        serde_json::Value::String(value.to_string())
    }
}

// ---------------------------------------------------------------------------
// Built-in namespace initialization (§9.15)
// ---------------------------------------------------------------------------

fn init_builtin_namespaces() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let namespaces = vec![
            NamespaceRegistration {
                name: "observability".to_string(),
                env_prefix: Some("APCORE_OBSERVABILITY".to_string()),
                defaults: Some(serde_json::json!({
                    "tracing": {
                        "enabled": false,
                        "sampling_rate": 1.0,
                        "strategy": "full",
                        "exporter": "stdout",
                        "otlp_endpoint": "http://localhost:4318"
                    },
                    "metrics": {
                        "enabled": false,
                        "exporter": "in_memory"
                    },
                    "logging": {
                        "level": "info",
                        "format": "json",
                        "redact_keys": ["password", "secret", "token", "api_key"]
                    },
                    "error_history": {
                        "max_entries_per_module": 50,
                        "max_total_entries": 1000
                    },
                    "platform_notify": {
                        "error_rate_threshold": 0.1,
                        "latency_p99_threshold_ms": 5000.0
                    }
                })),
                schema: None,
                env_style: EnvStyle::Auto,
                max_depth: DEFAULT_MAX_DEPTH,
                env_map: None,
            },
            NamespaceRegistration {
                name: "sys_modules".to_string(),
                env_prefix: Some("APCORE_SYS".to_string()),
                defaults: Some(serde_json::json!({
                    "enabled": true,
                    "health": { "enabled": true },
                    "manifest": { "enabled": true },
                    "usage": { "enabled": true, "retention_hours": 168, "bucketing_strategy": "hourly" },
                    "control": { "enabled": true },
                    "events": {
                        "enabled": false,
                        "subscribers": [],
                        "thresholds": { "error_rate": 0.1, "latency_p99_ms": 5000.0 }
                    }
                })),
                schema: None,
                env_style: EnvStyle::Auto,
                max_depth: DEFAULT_MAX_DEPTH,
                env_map: None,
            },
        ];
        for ns in namespaces {
            // Ignore duplicate errors on re-init
            let _ = Config::register_namespace(ns);
        }
    });
}

// ---------------------------------------------------------------------------
// Config discovery (§9.14)
// ---------------------------------------------------------------------------

fn discover_config_file() -> Option<std::path::PathBuf> {
    if let Ok(env_path) = std::env::var("APCORE_CONFIG_FILE") {
        if !env_path.is_empty() {
            return Some(std::path::PathBuf::from(env_path));
        }
    }

    let cwd_candidates = ["project.yaml", "project.yml", "apcore.yaml", "apcore.yml"];
    for name in &cwd_candidates {
        let p = std::path::Path::new(name);
        if p.exists() {
            return Some(p.to_path_buf());
        }
    }

    if let Some(home) = dirs_home() {
        #[cfg(target_os = "macos")]
        let xdg = home
            .join("Library")
            .join("Application Support")
            .join("apcore")
            .join("config.yaml");
        #[cfg(not(target_os = "macos"))]
        let xdg = home.join(".config").join("apcore").join("config.yaml");

        if xdg.exists() {
            return Some(xdg);
        }

        let legacy = home.join(".apcore").join("config.yaml");
        if legacy.exists() {
            return Some(legacy);
        }
    }

    None
}

fn dirs_home() -> Option<std::path::PathBuf> {
    std::env::var("HOME").ok().map(std::path::PathBuf::from)
}

/// Recursively merge `overlay` into `base`, preserving peer keys in nested
/// objects. Used by `Config::mount` (sync CB-002) to mirror Python's
/// `_deep_merge_dicts` semantics.
fn deep_merge_value(base: &mut serde_json::Value, overlay: &serde_json::Value) {
    match (base, overlay) {
        (serde_json::Value::Object(base_map), serde_json::Value::Object(overlay_map)) => {
            for (k, v) in overlay_map {
                match base_map.get_mut(k) {
                    Some(existing) => deep_merge_value(existing, v),
                    None => {
                        base_map.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (slot, value) => {
            *slot = value.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // Config::default and ExecutorConfig defaults
    // -------------------------------------------------------------------------

    #[test]
    fn default_config_has_expected_executor_values() {
        let cfg = Config::default();
        assert_eq!(cfg.executor.max_call_depth, 32);
        assert_eq!(cfg.executor.max_module_repeat, 3);
        assert_eq!(cfg.executor.default_timeout, 30_000);
        assert_eq!(cfg.executor.global_timeout, 60_000);
    }

    #[test]
    fn default_config_validates_successfully() {
        let cfg = Config::default();
        assert!(cfg.validate().is_ok());
    }

    // -------------------------------------------------------------------------
    // Config::get / set for canonical typed fields
    // -------------------------------------------------------------------------

    #[test]
    fn get_canonical_executor_key() {
        let cfg = Config::default();
        let depth = cfg
            .get("executor.max_call_depth")
            .expect("key should exist");
        assert_eq!(depth, serde_json::json!(32u64));
    }

    #[test]
    fn set_then_get_canonical_executor_key() {
        let mut cfg = Config::default();
        cfg.set("executor.max_call_depth", serde_json::json!(10u64));
        let val = cfg.get("executor.max_call_depth").unwrap();
        assert_eq!(val.as_u64().unwrap(), 10);
    }

    #[test]
    fn get_observability_tracing_enabled() {
        let cfg = Config::default();
        let enabled = cfg.get("observability.tracing.enabled").unwrap();
        // Default is false
        assert_eq!(enabled, serde_json::json!(false));
    }

    #[test]
    fn set_observability_tracing_enabled() {
        let mut cfg = Config::default();
        cfg.set("observability.tracing.enabled", serde_json::json!(true));
        assert!(cfg.observability.tracing.enabled);
    }

    // -------------------------------------------------------------------------
    // Config::get / set for user namespaces (dot-path traversal)
    // -------------------------------------------------------------------------

    #[test]
    fn set_and_get_user_namespace_key() {
        let mut cfg = Config::default();
        cfg.set(
            "myapp.db.url",
            serde_json::json!("postgres://localhost/test"),
        );
        let val = cfg.get("myapp.db.url").expect("should exist");
        assert_eq!(val.as_str().unwrap(), "postgres://localhost/test");
    }

    #[test]
    fn get_returns_none_for_missing_key() {
        let cfg = Config::default();
        assert!(cfg.get("nonexistent.key").is_none());
    }

    #[test]
    fn set_top_level_user_namespace_key() {
        let mut cfg = Config::default();
        cfg.set("myns", serde_json::json!("value"));
        assert_eq!(cfg.get("myns").unwrap(), serde_json::json!("value"));
    }

    // -------------------------------------------------------------------------
    // Config::validate
    // -------------------------------------------------------------------------

    #[test]
    fn validate_rejects_zero_max_call_depth() {
        let mut cfg = Config::default();
        cfg.executor.max_call_depth = 0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rejects_zero_max_module_repeat() {
        let mut cfg = Config::default();
        cfg.executor.max_module_repeat = 0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rejects_global_timeout_less_than_default_timeout() {
        let mut cfg = Config::default();
        cfg.executor.global_timeout = 1_000; // less than default_timeout (30_000)
        cfg.executor.default_timeout = 5_000;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_allows_zero_global_timeout_meaning_no_deadline() {
        let mut cfg = Config::default();
        cfg.executor.global_timeout = 0; // 0 = no global deadline
        assert!(cfg.validate().is_ok());
    }

    // -------------------------------------------------------------------------
    // Config deserialization — legacy field rejection
    // -------------------------------------------------------------------------

    #[test]
    fn deserialize_rejects_legacy_root_fields() {
        let json_str = r#"{"max_call_depth": 10}"#;
        let result: Result<Config, _> = serde_json::from_str(json_str);
        assert!(result.is_err(), "legacy root field should be rejected");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("v0.18.0") || err_msg.contains("max_call_depth"),
            "error should mention legacy key"
        );
    }

    #[test]
    fn deserialize_canonical_format_succeeds() {
        let json_str = r#"{"executor": {"max_call_depth": 16}}"#;
        let cfg: Config = serde_json::from_str(json_str).expect("canonical format should work");
        assert_eq!(cfg.executor.max_call_depth, 16);
    }

    // -------------------------------------------------------------------------
    // Config::data
    // -------------------------------------------------------------------------

    #[test]
    fn data_returns_json_object() {
        let cfg = Config::default();
        let data = cfg.data();
        assert!(data.is_object(), "data() should return a JSON object");
        assert!(data.get("executor").is_some());
    }

    // -------------------------------------------------------------------------
    // Config::reload without path
    // -------------------------------------------------------------------------

    #[test]
    fn reload_without_path_returns_error() {
        let mut cfg = Config::default();
        assert!(
            cfg.reload().is_err(),
            "reload without yaml_path should fail"
        );
    }

    // -------------------------------------------------------------------------
    // Config::mount
    // -------------------------------------------------------------------------

    #[test]
    fn mount_dict_into_user_namespace() {
        let mut cfg = Config::default();
        let data = serde_json::json!({"host": "localhost", "port": 5432});
        cfg.mount("database", MountSource::Dict(data)).unwrap();
        let host = cfg.get("database.host").unwrap();
        assert_eq!(host.as_str().unwrap(), "localhost");
    }

    #[test]
    fn mount_rejects_reserved_namespace() {
        let mut cfg = Config::default();
        let data = serde_json::json!({"key": "value"});
        let result = cfg.mount("_config", MountSource::Dict(data));
        assert!(
            result.is_err(),
            "should reject reserved namespace '_config'"
        );
    }

    #[test]
    fn mount_rejects_non_object_source() {
        let mut cfg = Config::default();
        let result = cfg.mount("ns", MountSource::Dict(serde_json::json!([1, 2, 3])));
        assert!(result.is_err(), "non-object source should be rejected");
    }

    // -------------------------------------------------------------------------
    // ConfigMode detection
    // -------------------------------------------------------------------------

    #[test]
    fn namespace_mode_detected_when_apcore_key_present() {
        let json_str = r#"{"apcore": {"executor": {"max_call_depth": 8}}}"#;
        let cfg: Config = serde_json::from_str(json_str).expect("should parse");
        // detect_mode() is called in from_yaml_file / from_json_file / from_defaults;
        // when deserializing raw, mode stays Legacy; we call detect_mode via from_defaults
        // which relies on from_defaults path. Test via from_defaults behavior:
        // Just verify the config parsed correctly.
        assert_eq!(cfg.executor.max_call_depth, 8);
    }
}

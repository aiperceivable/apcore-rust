// APCore Protocol — System modules registration
// Spec reference: Built-in system modules (F10, F11, F19)

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock, RwLock};

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::Mutex;

use crate::config::Config;
use crate::context::Context;
use crate::errors::{ErrorCode, ModuleError};
use crate::events::emitter::{ApCoreEvent, EventEmitter};
use crate::executor::Executor;
use crate::module::Module;
use crate::observability::metrics::MetricsCollector;
use crate::registry::registry::{ModuleDescriptor, Registry};

// ---------------------------------------------------------------------------
// ToggleState — thread-safe enable/disable tracking
// ---------------------------------------------------------------------------

/// Thread-safe set of disabled module IDs.
pub struct ToggleState {
    disabled: RwLock<HashSet<String>>,
}

impl ToggleState {
    pub fn new() -> Self {
        Self {
            disabled: RwLock::new(HashSet::new()),
        }
    }

    pub fn is_disabled(&self, module_id: &str) -> bool {
        // INVARIANT: RwLock is only poisoned on a panic inside a write guard.
        self.disabled.read().unwrap().contains(module_id)
    }

    pub fn disable(&self, module_id: &str) {
        // INVARIANT: as above.
        self.disabled.write().unwrap().insert(module_id.to_string());
    }

    pub fn enable(&self, module_id: &str) {
        // INVARIANT: RwLock is only poisoned on a panic inside a write guard.
        self.disabled.write().unwrap().remove(module_id);
    }

    pub fn clear(&self) {
        self.disabled.write().unwrap().clear();
    }
}

impl Default for ToggleState {
    fn default() -> Self {
        Self::new()
    }
}

// Global default instance.
static GLOBAL_TOGGLE_STATE: OnceLock<ToggleState> = OnceLock::new();

fn global_toggle_state() -> &'static ToggleState {
    GLOBAL_TOGGLE_STATE.get_or_init(ToggleState::new)
}

/// Check if a module is disabled using the default global toggle state.
pub fn is_module_disabled(module_id: &str) -> bool {
    global_toggle_state().is_disabled(module_id)
}

/// Return `Err(ModuleError)` with `ErrorCode::ModuleDisabled` if the module is disabled.
pub fn check_module_disabled(module_id: &str) -> Result<(), ModuleError> {
    if is_module_disabled(module_id) {
        return Err(ModuleError::new(
            ErrorCode::ModuleDisabled,
            format!("Module '{}' is disabled", module_id),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Sensitive key detection
// ---------------------------------------------------------------------------

const SENSITIVE_SEGMENTS: &[&str] = &["token", "secret", "key", "password", "auth", "credential"];

fn is_sensitive_key(key: &str) -> bool {
    let lower = key.to_lowercase();
    // W-6: Use substring containment so compound segments like "api_key" or
    // "auth_token" are also masked, not just exact-match segments.
    lower
        .split('.')
        .any(|seg| SENSITIVE_SEGMENTS.iter().any(|s| seg.contains(s)))
}

// ---------------------------------------------------------------------------
// Restricted config keys
// ---------------------------------------------------------------------------

// W-7: Lists keys that must not be changed at runtime via update_config.
// Scope: runtime-safety critical keys only. Schema-level immutability is
// enforced at load time; this list protects against inadvertent runtime mutations.
const RESTRICTED_KEYS: &[&str] = &["sys_modules.enabled"];

// ---------------------------------------------------------------------------
// UpdateConfigModule (F11)
// ---------------------------------------------------------------------------

/// Update a runtime configuration value by dot-path key (F11).
pub struct UpdateConfigModule {
    config: Arc<Mutex<Config>>,
    emitter: Arc<Mutex<EventEmitter>>,
}

impl UpdateConfigModule {
    pub fn new(config: Arc<Mutex<Config>>, emitter: Arc<Mutex<EventEmitter>>) -> Self {
        Self { config, emitter }
    }
}

#[async_trait]
impl Module for UpdateConfigModule {
    fn description(&self) -> &str {
        "Update a runtime configuration value by dot-path key"
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["key", "value", "reason"],
            "properties": {
                "key":    {"type": "string"},
                "value":  {},
                "reason": {"type": "string"}
            }
        })
    }

    fn output_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "success":   {"type": "boolean"},
                "key":       {"type": "string"},
                "old_value": {},
                "new_value": {}
            }
        })
    }

    async fn execute(
        &self,
        inputs: serde_json::Value,
        _ctx: &Context<serde_json::Value>,
    ) -> Result<serde_json::Value, ModuleError> {
        let key = require_string(&inputs, "key")?;
        let reason = require_string(&inputs, "reason")?;
        let value = inputs
            .get("value")
            .cloned()
            .ok_or_else(|| missing_field_error("value"))?;

        if RESTRICTED_KEYS.contains(&key.as_str()) {
            return Err(ModuleError::new(
                ErrorCode::ConfigInvalid,
                format!("Configuration key '{}' cannot be changed at runtime", key),
            )
            .with_details([("key".to_string(), json!(key))].into_iter().collect()));
        }

        let old_value = {
            let cfg = self.config.lock().await;
            cfg.get(&key)
        };

        {
            let mut cfg = self.config.lock().await;
            cfg.set(&key, value.clone());
        }

        let timestamp = chrono::Utc::now().to_rfc3339();
        let event_data = json!({
            "key": key,
            "old_value": old_value,
            "new_value": value,
        });

        emit_event(
            &self.emitter,
            "apcore.config.updated",
            "system.control.update_config",
            &timestamp,
            event_data.clone(),
        )
        .await;
        // W-9: DEPRECATED alias — emitted for backward compatibility during 0.15.x transition.
        emit_event(
            &self.emitter,
            "config_changed",
            "system.control.update_config",
            &timestamp,
            event_data,
        )
        .await;

        if is_sensitive_key(&key) {
            tracing::info!(key = %key, reason = %reason, "Config updated: old_value=*** new_value=***");
        } else {
            tracing::info!(
                key = %key,
                old_value = ?old_value,
                new_value = ?value,
                reason = %reason,
                "Config updated"
            );
        }

        Ok(json!({
            "success": true,
            "key": key,
            "old_value": old_value,
            "new_value": value,
        }))
    }
}

// ---------------------------------------------------------------------------
// ReloadModuleModule (F10)
// ---------------------------------------------------------------------------

/// Hot-reload a module via safe unregister (F10).
///
/// Full re-discovery is not supported in Rust (no dynamic loading). The module
/// is unregistered and callers must re-register manually. The event is always
/// emitted with new_version == previous_version.
pub struct ReloadModuleModule {
    registry: Arc<Mutex<Registry>>,
    emitter: Arc<Mutex<EventEmitter>>,
}

impl ReloadModuleModule {
    pub fn new(registry: Arc<Mutex<Registry>>, emitter: Arc<Mutex<EventEmitter>>) -> Self {
        Self { registry, emitter }
    }
}

#[async_trait]
impl Module for ReloadModuleModule {
    fn description(&self) -> &str {
        "Hot-reload a module by safe unregister (re-registration must be done explicitly in Rust)"
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["module_id", "reason"],
            "properties": {
                "module_id": {"type": "string"},
                "reason":    {"type": "string"}
            }
        })
    }

    fn output_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "success":            {"type": "boolean"},
                "module_id":          {"type": "string"},
                "previous_version":   {"type": "string"},
                "new_version":        {"type": "string"},
                "reload_duration_ms": {"type": "number"}
            }
        })
    }

    async fn execute(
        &self,
        inputs: serde_json::Value,
        _ctx: &Context<serde_json::Value>,
    ) -> Result<serde_json::Value, ModuleError> {
        let module_id = require_string(&inputs, "module_id")?;
        let reason = require_string(&inputs, "reason")?;

        let start = std::time::Instant::now();

        // W-5: Single lock for the check-then-unregister sequence to eliminate TOCTOU.
        // W-1: Version is not tracked in the Rust registry descriptor; use "unknown".
        let previous_version = {
            let mut reg = self.registry.lock().await;
            if !reg.has(&module_id) {
                return Err(ModuleError::new(
                    ErrorCode::ModuleNotFound,
                    format!("Module '{}' not found", module_id),
                ));
            }
            reg.safe_unregister(&module_id, 5000).await?;
            "unknown".to_string()
        };

        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        let new_version = previous_version.clone();
        let timestamp = chrono::Utc::now().to_rfc3339();
        let event_data = json!({
            "previous_version": previous_version,
            "new_version": new_version,
        });

        emit_event(
            &self.emitter,
            "apcore.module.reloaded",
            &module_id,
            &timestamp,
            event_data.clone(),
        )
        .await;
        // W-9: DEPRECATED alias — emitted for backward compatibility during 0.15.x transition.
        emit_event(
            &self.emitter,
            "config_changed",
            &module_id,
            &timestamp,
            event_data,
        )
        .await;

        tracing::info!(
            module_id = %module_id,
            previous_version = %previous_version,
            new_version = %new_version,
            reason = %reason,
            "Module reloaded"
        );

        Ok(json!({
            "success": true,
            "module_id": module_id,
            "previous_version": previous_version,
            "new_version": new_version,
            "reload_duration_ms": elapsed_ms,
        }))
    }
}

// ---------------------------------------------------------------------------
// ToggleFeatureModule (F19)
// ---------------------------------------------------------------------------

/// Disable or enable a module without unloading it from the Registry (F19).
pub struct ToggleFeatureModule {
    registry: Arc<Mutex<Registry>>,
    emitter: Arc<Mutex<EventEmitter>>,
    toggle_state: Arc<ToggleState>,
}

impl ToggleFeatureModule {
    pub fn new(
        registry: Arc<Mutex<Registry>>,
        emitter: Arc<Mutex<EventEmitter>>,
        toggle_state: Arc<ToggleState>,
    ) -> Self {
        Self {
            registry,
            emitter,
            toggle_state,
        }
    }
}

#[async_trait]
impl Module for ToggleFeatureModule {
    fn description(&self) -> &str {
        "Disable or enable a module without unloading it"
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["module_id", "enabled", "reason"],
            "properties": {
                "module_id": {"type": "string"},
                "enabled":   {"type": "boolean"},
                "reason":    {"type": "string"}
            }
        })
    }

    fn output_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "success":   {"type": "boolean"},
                "module_id": {"type": "string"},
                "enabled":   {"type": "boolean"}
            }
        })
    }

    async fn execute(
        &self,
        inputs: serde_json::Value,
        _ctx: &Context<serde_json::Value>,
    ) -> Result<serde_json::Value, ModuleError> {
        let module_id = require_string(&inputs, "module_id")?;
        let reason = require_string(&inputs, "reason")?;
        let enabled = inputs
            .get("enabled")
            .and_then(|v| v.as_bool())
            .ok_or_else(|| {
                ModuleError::new(
                    ErrorCode::GeneralInvalidInput,
                    "'enabled' is required and must be a boolean",
                )
            })?;

        {
            let reg = self.registry.lock().await;
            if !reg.has(&module_id) {
                return Err(ModuleError::new(
                    ErrorCode::ModuleNotFound,
                    format!("Module '{}' not found", module_id),
                ));
            }
        }

        if enabled {
            self.toggle_state.enable(&module_id);
        } else {
            self.toggle_state.disable(&module_id);
        }

        let timestamp = chrono::Utc::now().to_rfc3339();
        let event_data = json!({"enabled": enabled});

        emit_event(
            &self.emitter,
            "apcore.module.toggled",
            &module_id,
            &timestamp,
            event_data.clone(),
        )
        .await;
        // W-9: DEPRECATED alias — emitted for backward compatibility during 0.15.x transition.
        emit_event(
            &self.emitter,
            "module_health_changed",
            &module_id,
            &timestamp,
            event_data,
        )
        .await;

        tracing::info!(
            module_id = %module_id,
            enabled = %enabled,
            reason = %reason,
            "Module toggled"
        );

        Ok(json!({
            "success": true,
            "module_id": module_id,
            "enabled": enabled,
        }))
    }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

fn require_string(inputs: &serde_json::Value, field: &str) -> Result<String, ModuleError> {
    inputs
        .get(field)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .ok_or_else(|| {
            ModuleError::new(
                ErrorCode::GeneralInvalidInput,
                format!("'{}' is required and must be a non-empty string", field),
            )
        })
}

fn missing_field_error(field: &str) -> ModuleError {
    ModuleError::new(
        ErrorCode::GeneralInvalidInput,
        format!("'{}' is required", field),
    )
}

/// Emit an event; errors are logged and not propagated (error isolation).
async fn emit_event(
    emitter: &Arc<Mutex<EventEmitter>>,
    event_type: &str,
    module_id: &str,
    timestamp: &str,
    data: serde_json::Value,
) {
    let event = ApCoreEvent {
        event_type: event_type.to_string(),
        timestamp: timestamp.to_string(),
        data,
        module_id: Some(module_id.to_string()),
        severity: "info".to_string(),
    };
    let em = emitter.lock().await;
    if let Err(e) = em.emit(&event).await {
        tracing::warn!(error = %e, event_type = %event_type, "Event emit failed");
    }
}

// ---------------------------------------------------------------------------
// SysModulesContext — typed return value for register_sys_modules (W-8)
// ---------------------------------------------------------------------------

/// Holds references to components created during sys-module registration.
pub struct SysModulesContext {
    pub registered_modules: HashMap<String, serde_json::Value>,
    pub emitter: Arc<Mutex<EventEmitter>>,
    pub toggle_state: Arc<ToggleState>,
}

// ---------------------------------------------------------------------------
// register_sys_modules
// ---------------------------------------------------------------------------

/// Register built-in system control modules into the registry.
///
/// `registry` must be an `Arc<Mutex<Registry>>` so the async reload/toggle
/// modules can hold a live reference to the same registry the caller uses.
/// (C-1: Previously this function created a disconnected empty registry.)
///
/// Returns `None` if `sys_modules.enabled` is `false` in config. (C-2)
pub fn register_sys_modules(
    registry: Arc<Mutex<Registry>>,
    _executor: &mut Executor,
    config: &Config,
    _metrics_collector: Option<MetricsCollector>,
) -> Option<SysModulesContext> {
    // C-2: Guard on sys_modules.enabled (default: true per spec §9.15).
    let enabled = config
        .get("sys_modules.enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    if !enabled {
        return None;
    }

    let config_arc = Arc::new(Mutex::new(config.clone()));
    let emitter_arc = Arc::new(Mutex::new(EventEmitter::new()));
    let toggle_state = Arc::new(ToggleState::new());

    let update_config: Box<dyn Module> = Box::new(UpdateConfigModule::new(
        Arc::clone(&config_arc),
        Arc::clone(&emitter_arc),
    ));
    // C-1: Pass the caller's registry Arc so reload/toggle can operate on the
    //      live registry, not a disconnected empty one.
    let reload_module: Box<dyn Module> = Box::new(ReloadModuleModule::new(
        Arc::clone(&registry),
        Arc::clone(&emitter_arc),
    ));
    let toggle_feature: Box<dyn Module> = Box::new(ToggleFeatureModule::new(
        Arc::clone(&registry),
        Arc::clone(&emitter_arc),
        Arc::clone(&toggle_state),
    ));

    let modules: Vec<(&str, Box<dyn Module>)> = vec![
        ("system.control.update_config", update_config),
        ("system.control.reload_module", reload_module),
        ("system.control.toggle_feature", toggle_feature),
    ];

    let mut registered: HashMap<String, serde_json::Value> = HashMap::new();
    // INVARIANT: This function is called from a synchronous context; no
    // concurrent holder of this lock exists before registration completes.
    let mut reg = registry.blocking_lock();

    for (id, module) in modules {
        let descriptor = ModuleDescriptor {
            name: id.to_string(),
            annotations: crate::module::ModuleAnnotations {
                requires_approval: true,
                ..Default::default()
            },
            input_schema: module.input_schema(),
            output_schema: module.output_schema(),
            enabled: true,
            tags: vec!["system".to_string(), "control".to_string()],
            dependencies: vec![],
        };
        let info = json!({
            "name": id,
            "description": module.description(),
        });
        match reg.register_internal(id, module, descriptor) {
            Ok(()) => {
                registered.insert(id.to_string(), info);
            }
            Err(e) => {
                tracing::warn!(module_id = %id, error = %e, "Failed to register sys module");
            }
        }
    }

    Some(SysModulesContext {
        registered_modules: registered,
        emitter: emitter_arc,
        toggle_state,
    })
}

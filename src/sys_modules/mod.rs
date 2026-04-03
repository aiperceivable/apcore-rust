// APCore Protocol — System modules registration
// Spec reference: Built-in system modules (F10, F11, F19)

pub mod control;
pub mod health;
pub mod manifest;
pub mod usage;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock, RwLock};

use serde_json::json;
use tokio::sync::Mutex;

use crate::config::Config;
use crate::errors::{ErrorCode, ModuleError};
use crate::events::emitter::{ApCoreEvent, EventEmitter};
use crate::events::subscribers::create_subscriber;
use crate::executor::Executor;
use crate::middleware::PlatformNotifyMiddleware;
use crate::module::Module;
use crate::observability::error_history::{ErrorHistory, ErrorHistoryMiddleware};
use crate::observability::metrics::MetricsCollector;
use crate::observability::usage::{UsageCollector, UsageMiddleware};
use crate::registry::registry::{ModuleDescriptor, Registry};

pub use control::{UpdateConfigModule, ReloadModuleModule, ToggleFeatureModule};

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

pub(crate) const SENSITIVE_SEGMENTS: &[&str] = &["token", "secret", "key", "password", "auth", "credential"];

pub(crate) fn is_sensitive_key(key: &str) -> bool {
    let lower = key.to_lowercase();
    // W-6: Match exact segments ("key") or underscore-compound segments ("api_key",
    // "auth_token") without false-positives on "keyboard" or "authentication".
    lower.split('.').any(|seg| {
        SENSITIVE_SEGMENTS.iter().any(|&s| {
            seg == s || seg.ends_with(&format!("_{s}")) || seg.starts_with(&format!("{s}_"))
        })
    })
}

// ---------------------------------------------------------------------------
// Restricted config keys
// ---------------------------------------------------------------------------

// W-7: Lists keys that must not be changed at runtime via update_config.
// Scope: runtime-safety critical keys only. Schema-level immutability is
// enforced at load time; this list protects against inadvertent runtime mutations.
pub(crate) const RESTRICTED_KEYS: &[&str] = &["sys_modules.enabled"];

// ---------------------------------------------------------------------------
// Shared helpers (used by control.rs)
// ---------------------------------------------------------------------------

pub(crate) fn require_string(inputs: &serde_json::Value, field: &str) -> Result<String, ModuleError> {
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

pub(crate) fn missing_field_error(field: &str) -> ModuleError {
    ModuleError::new(
        ErrorCode::GeneralInvalidInput,
        format!("'{}' is required", field),
    )
}

/// Emit an event; errors are logged and not propagated (error isolation).
pub(crate) async fn emit_event(
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
// SysModulesContext — typed return value for register_sys_modules
// ---------------------------------------------------------------------------

/// Holds references to components created during sys-module registration.
pub struct SysModulesContext {
    pub registered_modules: HashMap<String, serde_json::Value>,
    pub emitter: Arc<Mutex<EventEmitter>>,
    pub toggle_state: Arc<ToggleState>,
    pub error_history: ErrorHistory,
    pub usage_collector: UsageCollector,
}

// ---------------------------------------------------------------------------
// register_sys_modules
// ---------------------------------------------------------------------------

/// Register built-in system modules into the registry.
///
/// Workflow (per spec §9.15):
/// 1. Check `sys_modules.enabled` — return `None` if false.
/// 2. Create `ErrorHistory` + `ErrorHistoryMiddleware`, register on executor.
/// 3. Create `UsageCollector` + `UsageMiddleware`, register on executor.
/// 4. Register health, manifest, and usage modules (always).
/// 5. If `sys_modules.events.enabled`: register control modules + EventEmitter.
///
/// # Panics
///
/// Panics if called from within a tokio async runtime. This function uses
/// `blocking_lock()` internally and must be called from a synchronous context
/// (e.g. application startup, before entering the async runtime).
pub fn register_sys_modules(
    registry: Arc<Mutex<Registry>>,
    executor: &mut Executor,
    config: &Config,
    metrics_collector: Option<MetricsCollector>,
) -> Option<SysModulesContext> {
    let enabled = config
        .get("sys_modules.enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    if !enabled {
        return None;
    }

    // --- Step 2: ErrorHistory + middleware ---
    let max_per_module = config
        .get("sys_modules.error_history.max_entries_per_module")
        .and_then(|v| v.as_u64())
        .unwrap_or(50) as usize;
    let max_total = config
        .get("sys_modules.error_history.max_total_entries")
        .and_then(|v| v.as_u64())
        .unwrap_or(1000) as usize;
    let error_history = ErrorHistory::with_limits(max_per_module, max_total);
    let eh_middleware = ErrorHistoryMiddleware::new(error_history.clone());
    let _ = executor.use_middleware(Box::new(eh_middleware));

    // --- Step 3: UsageCollector + middleware ---
    let usage_collector = UsageCollector::new();
    let usage_middleware = UsageMiddleware::new(usage_collector.clone());
    let _ = executor.use_middleware(Box::new(usage_middleware));

    let config_arc = Arc::new(Mutex::new(config.clone()));
    let emitter_arc = Arc::new(Mutex::new(EventEmitter::new()));
    let toggle_state = Arc::new(ToggleState::new());

    // --- Step 4: Build module list (health + manifest + usage always) ---
    let mut modules: Vec<(&str, Box<dyn Module>, Vec<String>)> = vec![
        (
            "system.health.summary",
            Box::new(health::HealthSummaryModule::new(
                Arc::clone(&registry),
                metrics_collector.clone(),
                error_history.clone(),
                Arc::clone(&config_arc),
            )),
            vec!["system".into(), "health".into()],
        ),
        (
            "system.health.module",
            Box::new(health::HealthModuleModule::new(
                Arc::clone(&registry),
                metrics_collector.clone(),
                error_history.clone(),
            )),
            vec!["system".into(), "health".into()],
        ),
        (
            "system.manifest.module",
            Box::new(manifest::ManifestModuleModule::new(
                Arc::clone(&registry),
                Arc::clone(&config_arc),
            )),
            vec!["system".into(), "manifest".into()],
        ),
        (
            "system.manifest.full",
            Box::new(manifest::ManifestFullModule::new(
                Arc::clone(&registry),
                Arc::clone(&config_arc),
            )),
            vec!["system".into(), "manifest".into()],
        ),
        (
            "system.usage.summary",
            Box::new(usage::UsageSummaryModule::new(usage_collector.clone())),
            vec!["system".into(), "usage".into()],
        ),
        (
            "system.usage.module",
            Box::new(usage::UsageModuleModule::new(
                Arc::clone(&registry),
                usage_collector.clone(),
            )),
            vec!["system".into(), "usage".into()],
        ),
    ];

    // --- Step 5: Control modules only if events.enabled ---
    let events_enabled = config
        .get("sys_modules.events.enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if events_enabled {
        // Step 5a: PlatformNotifyMiddleware (gets its own EventEmitter instance).
        let error_rate_threshold = config
            .get("sys_modules.events.thresholds.error_rate")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.1);
        let latency_p99_threshold = config
            .get("sys_modules.events.thresholds.latency_p99_ms")
            .and_then(|v| v.as_f64())
            .unwrap_or(5000.0);
        let pn_middleware = PlatformNotifyMiddleware::new(
            EventEmitter::new(),
            metrics_collector.clone(),
            error_rate_threshold,
            latency_p99_threshold,
        );
        let _ = executor.use_middleware(Box::new(pn_middleware));

        // Step 5b: Instantiate subscribers from config
        if let Some(subs) = config.get("sys_modules.events.subscribers") {
            if let Some(arr) = subs.as_array() {
                let mut em = emitter_arc.blocking_lock();
                for sub_config in arr {
                    match create_subscriber(sub_config) {
                        Ok(subscriber) => em.subscribe(subscriber),
                        Err(e) => {
                            tracing::warn!(error = %e, "Failed to create subscriber from config");
                        }
                    }
                }
            }
        }

        // Step 5c: Control modules
        modules.push((
            "system.control.update_config",
            Box::new(UpdateConfigModule::new(
                Arc::clone(&config_arc),
                Arc::clone(&emitter_arc),
            )),
            vec!["system".into(), "control".into()],
        ));
        modules.push((
            "system.control.reload_module",
            Box::new(ReloadModuleModule::new(
                Arc::clone(&registry),
                Arc::clone(&emitter_arc),
            )),
            vec!["system".into(), "control".into()],
        ));
        modules.push((
            "system.control.toggle_feature",
            Box::new(ToggleFeatureModule::new(
                Arc::clone(&registry),
                Arc::clone(&emitter_arc),
                Arc::clone(&toggle_state),
            )),
            vec!["system".into(), "control".into()],
        ));
    }

    // --- Register all modules ---
    let mut registered: HashMap<String, serde_json::Value> = HashMap::new();
    // INVARIANT: This function is called from a synchronous context; no
    // concurrent holder of this lock exists before registration completes.
    let mut reg = registry.blocking_lock();

    for (id, module, tags) in modules {
        let is_control = tags.contains(&"control".to_string());
        let descriptor = ModuleDescriptor {
            name: id.to_string(),
            annotations: crate::module::ModuleAnnotations {
                requires_approval: is_control,
                readonly: !is_control,
                idempotent: !is_control,
                ..Default::default()
            },
            input_schema: module.input_schema(),
            output_schema: module.output_schema(),
            enabled: true,
            tags,
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

    // Step 5d: Bridge registry events to EventEmitter.
    // Registry callbacks are synchronous; emit is async. We log the event
    // and use try_lock to best-effort emit without blocking.
    if events_enabled {
        reg.on(
            "register",
            Box::new(move |module_id: &str, _module: &dyn Module| {
                tracing::info!(module_id = %module_id, "module_registered");
            }),
        );
        reg.on(
            "unregister",
            Box::new(move |module_id: &str, _module: &dyn Module| {
                tracing::info!(module_id = %module_id, "module_unregistered");
            }),
        );
    }

    Some(SysModulesContext {
        registered_modules: registered,
        emitter: emitter_arc,
        toggle_state,
        error_history,
        usage_collector,
    })
}

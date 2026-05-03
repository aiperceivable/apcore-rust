// APCore Protocol — System modules registration
// Spec reference: Built-in system modules (F10, F11, F19) +
//                 system-modules.md §1.1–§1.5 hardening (Issue #45).

pub mod audit;
pub mod control;
pub mod health;
pub mod manifest;
pub mod overrides;
pub mod usage;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use parking_lot::RwLock;

use serde_json::json;
use thiserror::Error;
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

pub use audit::{AuditAction, AuditChange, AuditEntry, AuditStore, InMemoryAuditStore};
pub use control::UpdateConfigModule;
pub(crate) use control::{ReloadModule, ToggleFeatureModule};

// ---------------------------------------------------------------------------
// ToggleState — thread-safe enable/disable tracking
// ---------------------------------------------------------------------------

/// Thread-safe set of disabled module IDs.
pub struct ToggleState {
    disabled: RwLock<HashSet<String>>,
}

impl ToggleState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            disabled: RwLock::new(HashSet::new()),
        }
    }

    pub fn is_disabled(&self, module_id: &str) -> bool {
        self.disabled.read().contains(module_id)
    }

    pub fn disable(&self, module_id: &str) {
        self.disabled.write().insert(module_id.to_string());
    }

    pub fn enable(&self, module_id: &str) {
        self.disabled.write().remove(module_id);
    }

    pub fn clear(&self) {
        self.disabled.write().clear();
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
#[must_use]
pub fn is_module_disabled(module_id: &str) -> bool {
    global_toggle_state().is_disabled(module_id)
}

/// Return `Err(ModuleError)` with `ErrorCode::ModuleDisabled` if the module is disabled.
pub fn check_module_disabled(module_id: &str) -> Result<(), ModuleError> {
    if is_module_disabled(module_id) {
        return Err(ModuleError::new(
            ErrorCode::ModuleDisabled,
            format!("Module '{module_id}' is disabled"),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Sensitive key detection
// ---------------------------------------------------------------------------

pub(crate) const SENSITIVE_SEGMENTS: &[&str] =
    &["token", "secret", "key", "password", "auth", "credential"];

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

pub(crate) fn require_string(
    inputs: &serde_json::Value,
    field: &str,
) -> Result<String, ModuleError> {
    inputs
        .get(field)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(std::string::ToString::to_string)
        .ok_or_else(|| {
            ModuleError::new(
                ErrorCode::GeneralInvalidInput,
                format!("'{field}' is required and must be a non-empty string"),
            )
        })
}

pub(crate) fn missing_field_error(field: &str) -> ModuleError {
    ModuleError::new(
        ErrorCode::GeneralInvalidInput,
        format!("'{field}' is required"),
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

/// Default `caller_id` when the `Context` has none (Issue #45.2 — contextual
/// auditing). Cross-language parity: `apcore-python` and `apcore-typescript`
/// both fall back to the literal `"@external"` string.
pub(crate) const DEFAULT_EXTERNAL_CALLER: &str = "@external";

/// Augment an audit-event payload with caller identity extracted from the
/// `Context` (Issue #45.2). Adds:
///   * `caller_id` — taken from `ctx.caller_id`, defaulted to `"@external"`.
///   * `actor_id` / `actor_type` — taken from `ctx.identity` when present.
///
/// The `data` argument is mutated in place and returned for ergonomic chaining.
pub(crate) fn augment_with_context_identity(
    mut data: serde_json::Value,
    ctx: &crate::context::Context<serde_json::Value>,
) -> serde_json::Value {
    if let Some(obj) = data.as_object_mut() {
        let caller_id = ctx
            .caller_id
            .clone()
            .unwrap_or_else(|| DEFAULT_EXTERNAL_CALLER.to_string());
        obj.insert("caller_id".to_string(), serde_json::json!(caller_id));
        if let Some(identity) = ctx.identity.as_ref() {
            obj.insert("actor_id".to_string(), serde_json::json!(identity.id()));
            obj.insert(
                "actor_type".to_string(),
                serde_json::json!(identity.identity_type()),
            );
        }
    }
    data
}

// ---------------------------------------------------------------------------
// SysModuleError — strict registration failure (Issue #45 §1.5)
// ---------------------------------------------------------------------------

/// Error type returned by `register_sys_modules`. The `RegistrationFailed`
/// variant carries the offending `module_id` so callers can route the failure
/// to module-specific recovery logic.
#[derive(Debug, Error)]
pub enum SysModuleError {
    #[error("system module '{module_id}' failed to register: {source}")]
    RegistrationFailed {
        module_id: String,
        #[source]
        source: ModuleError,
    },
}

impl SysModuleError {
    #[must_use]
    pub fn module_id(&self) -> &str {
        match self {
            Self::RegistrationFailed { module_id, .. } => module_id,
        }
    }

    #[must_use]
    pub fn error_code(&self) -> ErrorCode {
        ErrorCode::SysModuleRegistrationFailed
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
    pub audit_store: Option<Arc<dyn AuditStore>>,
}

// ---------------------------------------------------------------------------
// SysModulesOptions — optional inputs for hardening features (§1.1, §1.2, §1.5)
// ---------------------------------------------------------------------------

/// Optional knobs accepted by [`register_sys_modules_with_options`].
///
/// Defaults preserve pre-hardening behavior: no overrides file, no audit store,
/// `fail_on_error: false`. Cross-language: maps to the `audit_store`,
/// `overrides_path`, and `fail_on_error` parameters in the Python and
/// TypeScript SDKs.
#[derive(Default, Clone)]
pub struct SysModulesOptions {
    /// When set, runtime overrides are loaded from this YAML path on startup
    /// and persisted on every `update_config` / `toggle_feature` call.
    pub overrides_path: Option<PathBuf>,
    /// When set, every state-changing control call appends an `AuditEntry`.
    /// When `None`, audit entries are logged at INFO level and discarded.
    pub audit_store: Option<Arc<dyn AuditStore>>,
    /// When `true`, any module-registration failure halts startup with an
    /// `Err(SysModuleError::RegistrationFailed)`. Default is `false`, which
    /// matches the lenient behavior of the Python/TypeScript SDKs.
    pub fail_on_error: bool,
}

// ---------------------------------------------------------------------------
// register_sys_modules — main entry, breaking change in 0.20.0 (§1.5)
// ---------------------------------------------------------------------------

/// Register built-in system modules into the registry.
///
/// **Breaking change in 0.20.0** (system-modules.md §1.5): the return type is
/// now `Result<SysModulesContext, SysModuleError>`. When `sys_modules.enabled`
/// is `false`, the function returns `Ok(SysModulesContext { … empty … })`
/// rather than `Option::None` so callers can always destructure the value.
///
/// For overrides persistence and audit trails, use
/// [`register_sys_modules_with_options`].
///
/// Workflow (per spec §9.15):
/// 1. Check `sys_modules.enabled` — return an empty `SysModulesContext` if `false`.
/// 2. Load `overrides_path` (if any) into the live `Config` after the base load.
/// 3. Create `ErrorHistory` + `ErrorHistoryMiddleware`.
/// 4. Create `UsageCollector` + `UsageMiddleware`.
/// 5. Register health, manifest, and usage modules.
/// 6. If `sys_modules.events.enabled`: register control modules + `EventEmitter`.
pub fn register_sys_modules(
    registry: Arc<Registry>,
    executor: &Executor,
    config: &Config,
    metrics_collector: Option<MetricsCollector>,
) -> Result<SysModulesContext, SysModuleError> {
    register_sys_modules_with_options(
        registry,
        executor,
        config,
        metrics_collector,
        SysModulesOptions::default(),
    )
}

/// Variant of [`register_sys_modules`] accepting hardening options. See
/// [`SysModulesOptions`] for details.
#[allow(clippy::too_many_lines)] // complex orchestration; extraction would obscure the registration flow
#[allow(clippy::needless_pass_by_value)] // public API: Arc<Registry> and Option<MetricsCollector> consumed by sub-modules
pub fn register_sys_modules_with_options(
    registry: Arc<Registry>,
    executor: &Executor,
    config: &Config,
    metrics_collector: Option<MetricsCollector>,
    options: SysModulesOptions,
) -> Result<SysModulesContext, SysModuleError> {
    let SysModulesOptions {
        overrides_path,
        audit_store,
        fail_on_error,
    } = options;

    let toggle_state = Arc::new(ToggleState::new());

    let enabled = config
        .get("sys_modules.enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    if !enabled {
        return Ok(SysModulesContext {
            registered_modules: HashMap::new(),
            emitter: Arc::new(Mutex::new(EventEmitter::new())),
            toggle_state,
            error_history: ErrorHistory::with_limits(50, 1000),
            usage_collector: UsageCollector::new(),
            audit_store,
        });
    }

    // --- §1.1: load overrides into a mutable Config clone, then share it ---
    let mut effective_config = config.clone();
    if let Some(path) = overrides_path.as_deref() {
        overrides::load_overrides(path, &mut effective_config, Some(&toggle_state));
    }

    // --- Step 2: ErrorHistory + middleware ---
    #[allow(clippy::cast_possible_truncation)] // config value won't exceed platform usize limits
    let max_per_module = effective_config
        .get("sys_modules.error_history.max_entries_per_module")
        .and_then(|v| v.as_u64())
        .unwrap_or(50) as usize;
    #[allow(clippy::cast_possible_truncation)] // config value won't exceed platform usize limits
    let max_total = effective_config
        .get("sys_modules.error_history.max_total_entries")
        .and_then(|v| v.as_u64())
        .unwrap_or(1000) as usize;
    let error_history = ErrorHistory::with_limits(max_per_module, max_total);
    let eh_middleware = ErrorHistoryMiddleware::new(error_history.clone());
    if let Err(e) = executor.use_middleware(Box::new(eh_middleware)) {
        tracing::error!(error = %e, middleware = "ErrorHistoryMiddleware", "sys middleware registration failed");
    }

    // --- Step 3: UsageCollector + middleware ---
    let usage_collector = UsageCollector::new();
    let usage_middleware = UsageMiddleware::new(usage_collector.clone());
    if let Err(e) = executor.use_middleware(Box::new(usage_middleware)) {
        tracing::error!(error = %e, middleware = "UsageMiddleware", "sys middleware registration failed");
    }

    let config_arc = Arc::new(Mutex::new(effective_config.clone()));

    // Build the EventEmitter up-front as an owned value so we can populate
    // its subscribers from config synchronously, then wrap it in the Arc<Mutex<_>>
    // shared with sys modules.
    let mut emitter = EventEmitter::new();

    let events_enabled = effective_config
        .get("sys_modules.events.enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // §1.1 + §1.2 wire `overrides_path` and `audit_store` through the control
    // modules — but control modules are only registered when events are
    // enabled. Surface a warning instead of silently dropping the options so
    // misconfiguration shows up at startup, not as missing audit entries.
    if !events_enabled && (overrides_path.is_some() || audit_store.is_some()) {
        tracing::warn!(
            overrides_path_set = overrides_path.is_some(),
            audit_store_set = audit_store.is_some(),
            "SysModulesOptions.overrides_path / audit_store are set but \
             sys_modules.events.enabled=false — control modules are not \
             registered, so these options have no effect. Enable events to \
             activate runtime overrides and audit trails."
        );
    }

    if events_enabled {
        // Instantiate subscribers from config while we still own `emitter`
        // directly — no lock required.
        if let Some(subs) = effective_config.get("sys_modules.events.subscribers") {
            if let Some(arr) = subs.as_array() {
                for sub_config in arr {
                    match create_subscriber(sub_config) {
                        Ok(subscriber) => emitter.subscribe(subscriber),
                        Err(e) => {
                            tracing::warn!(error = %e, "Failed to create subscriber from config");
                        }
                    }
                }
            }
        }
    }

    let emitter_arc = Arc::new(Mutex::new(emitter));

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
            Box::new(health::HealthModule::new(
                Arc::clone(&registry),
                metrics_collector.clone(),
                error_history.clone(),
            )),
            vec!["system".into(), "health".into()],
        ),
        (
            "system.manifest.module",
            Box::new(manifest::ManifestModule::new(
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
            Box::new(usage::UsageModule::new(
                Arc::clone(&registry),
                usage_collector.clone(),
            )),
            vec!["system".into(), "usage".into()],
        ),
    ];

    // --- Step 5: Control modules only if events.enabled ---
    if events_enabled {
        let error_rate_threshold = effective_config
            .get("sys_modules.events.thresholds.error_rate")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.1);
        let latency_p99_threshold = effective_config
            .get("sys_modules.events.thresholds.latency_p99_ms")
            .and_then(|v| v.as_f64())
            .unwrap_or(5000.0);
        let pn_middleware = PlatformNotifyMiddleware::new(
            EventEmitter::new(),
            metrics_collector.clone(),
            error_rate_threshold,
            latency_p99_threshold,
        );
        if let Err(e) = executor.use_middleware(Box::new(pn_middleware)) {
            tracing::error!(error = %e, middleware = "PlatformNotifyMiddleware", "sys middleware registration failed");
        }

        modules.push((
            "system.control.update_config",
            Box::new(
                UpdateConfigModule::new(Arc::clone(&config_arc), Arc::clone(&emitter_arc))
                    .with_overrides_path(overrides_path.clone())
                    .with_audit_store(audit_store.clone()),
            ),
            vec!["system".into(), "control".into()],
        ));
        modules.push((
            "system.control.reload_module",
            Box::new(
                ReloadModule::new(Arc::clone(&registry), Arc::clone(&emitter_arc))
                    .with_audit_store(audit_store.clone()),
            ),
            vec!["system".into(), "control".into()],
        ));
        modules.push((
            "system.control.toggle_feature",
            Box::new(
                ToggleFeatureModule::new(
                    Arc::clone(&registry),
                    Arc::clone(&emitter_arc),
                    Arc::clone(&toggle_state),
                )
                .with_overrides_path(overrides_path.clone())
                .with_audit_store(audit_store.clone()),
            ),
            vec!["system".into(), "control".into()],
        ));
    }

    // --- Register all modules ---
    let mut registered: HashMap<String, serde_json::Value> = HashMap::new();

    for (id, module, tags) in modules {
        let is_control = tags.contains(&"control".to_string());
        let descriptor = ModuleDescriptor {
            module_id: id.to_string(),
            name: None,
            description: module.description().to_string(),
            documentation: None,
            input_schema: module.input_schema(),
            output_schema: module.output_schema(),
            version: "1.0.0".to_string(),
            tags,
            annotations: Some(crate::module::ModuleAnnotations {
                requires_approval: is_control,
                readonly: !is_control,
                idempotent: !is_control,
                ..Default::default()
            }),
            examples: vec![],
            metadata: std::collections::HashMap::new(),
            display: None,
            sunset_date: None,
            dependencies: vec![],
            enabled: true,
        };
        let info = json!({
            "name": id,
            "description": module.description(),
        });
        match registry.register_internal(id, module, descriptor) {
            Ok(()) => {
                registered.insert(id.to_string(), info);
            }
            Err(e) => {
                if fail_on_error {
                    return Err(SysModuleError::RegistrationFailed {
                        module_id: id.to_string(),
                        source: e,
                    });
                }
                tracing::error!(module_id = %id, error = %e, "System module failed to register; continuing");
            }
        }
    }

    // Step 5d: Bridge registry events to ApCoreEvents (Issue #36).
    //
    // Each registry hook dual-emits the canonical
    // `apcore.registry.<event>` name AND the legacy bare-name event
    // (`module_registered`, `module_unregistered`) so existing subscribers
    // continue to fire while consumers migrate to the canonical names. The
    // legacy event payload includes `deprecated: true`.
    //
    // Registry callbacks are synchronous, so each hook spawns a task to
    // dispatch the async emit — fire-and-forget, error-isolated.
    if events_enabled {
        let emitter_for_register = Arc::clone(&emitter_arc);
        registry.on(
            "register",
            Box::new(move |module_id: &str, _module: &dyn Module| {
                tracing::info!(module_id = %module_id, "module_registered");
                let emitter = Arc::clone(&emitter_for_register);
                let module_id_owned = module_id.to_string();
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    handle.spawn(async move {
                        let canonical = ApCoreEvent::with_module(
                            "apcore.registry.module_registered",
                            json!({}),
                            &module_id_owned,
                            "info",
                        );
                        let legacy = ApCoreEvent::with_module(
                            "module_registered",
                            json!({
                                "deprecated": true,
                                "canonical_event": "apcore.registry.module_registered",
                            }),
                            &module_id_owned,
                            "info",
                        );
                        let em = emitter.lock().await;
                        let _ = em.emit(&canonical).await;
                        let _ = em.emit(&legacy).await;
                    });
                }
            }),
        );
        let emitter_for_unregister = Arc::clone(&emitter_arc);
        registry.on(
            "unregister",
            Box::new(move |module_id: &str, _module: &dyn Module| {
                tracing::info!(module_id = %module_id, "module_unregistered");
                let emitter = Arc::clone(&emitter_for_unregister);
                let module_id_owned = module_id.to_string();
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    handle.spawn(async move {
                        let canonical = ApCoreEvent::with_module(
                            "apcore.registry.module_unregistered",
                            json!({}),
                            &module_id_owned,
                            "info",
                        );
                        let legacy = ApCoreEvent::with_module(
                            "module_unregistered",
                            json!({
                                "deprecated": true,
                                "canonical_event": "apcore.registry.module_unregistered",
                            }),
                            &module_id_owned,
                            "info",
                        );
                        let em = emitter.lock().await;
                        let _ = em.emit(&canonical).await;
                        let _ = em.emit(&legacy).await;
                    });
                }
            }),
        );
    }

    Ok(SysModulesContext {
        registered_modules: registered,
        emitter: emitter_arc,
        toggle_state,
        error_history,
        usage_collector,
        audit_store,
    })
}

// APCore Protocol — System control modules
// Spec reference: system.control.update_config (F11), system.control.reload_module (F10),
//                 system.control.toggle_feature (F19)
// Hardening (Issue #45 / system-modules.md §1.1–§1.4):
//   §1.1 — overrides_path persistence for update_config + toggle_feature
//   §1.2 — contextual AuditEntry recorded for every state-changing call
//   §1.4 — path_filter glob (mutually exclusive with module_id) and
//          dependency-topological reload order

use async_trait::async_trait;
use glob::Pattern;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::config::Config;
use crate::context::Context;
use crate::errors::{ErrorCode, ModuleError};
use crate::events::emitter::EventEmitter;
use crate::module::Module;
use crate::observability::redaction::DEFAULT_REPLACEMENT;
use crate::registry::dependencies::resolve_dependencies;
use crate::registry::registry::Registry;
use crate::registry::types::DepInfo;

use super::audit::{build_audit_entry, record_audit, AuditAction, AuditChange, AuditStore};
use super::overrides::write_override;
use super::{
    emit_event, is_sensitive_key, missing_field_error, require_string, ToggleState, RESTRICTED_KEYS,
};

// ---------------------------------------------------------------------------
// UpdateConfigModule (F11) — runtime config mutation with optional persistence
// ---------------------------------------------------------------------------

/// Update a runtime configuration value by dot-path key (F11).
pub struct UpdateConfigModule {
    config: Arc<Mutex<Config>>,
    emitter: Arc<Mutex<EventEmitter>>,
    overrides_path: Option<PathBuf>,
    audit_store: Option<Arc<dyn AuditStore>>,
}

impl UpdateConfigModule {
    pub fn new(config: Arc<Mutex<Config>>, emitter: Arc<Mutex<EventEmitter>>) -> Self {
        Self {
            config,
            emitter,
            overrides_path: None,
            audit_store: None,
        }
    }

    #[must_use]
    pub fn with_overrides_path(mut self, overrides_path: Option<PathBuf>) -> Self {
        self.overrides_path = overrides_path;
        self
    }

    #[must_use]
    pub fn with_audit_store(mut self, audit_store: Option<Arc<dyn AuditStore>>) -> Self {
        self.audit_store = audit_store;
        self
    }
}

#[async_trait]
impl Module for UpdateConfigModule {
    fn description(&self) -> &'static str {
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
        ctx: &Context<serde_json::Value>,
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
                format!("Configuration key '{key}' cannot be changed at runtime"),
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

        // Persist to overrides.yaml *after* the in-memory mutation succeeded so
        // a write failure cannot poison the runtime state. Errors are logged
        // and not propagated — overrides persistence is best-effort.
        if let Some(path) = self.overrides_path.as_deref() {
            write_override(path, &key, &value);
        }

        // §1.2 + spec §F11 lines 337-339: redact `old_value`/`new_value` in the
        // emitted event, the audit entry, and the response payload when `key`
        // matches a sensitive segment. The in-memory `Config` still holds the
        // real value — the sentinel only blocks egress to logs / events / audit
        // store / RPC response.
        let sensitive = is_sensitive_key(&key);
        let redacted_old: serde_json::Value = if sensitive {
            json!(DEFAULT_REPLACEMENT)
        } else {
            old_value.clone().unwrap_or(serde_json::Value::Null)
        };
        let redacted_new: serde_json::Value = if sensitive {
            json!(DEFAULT_REPLACEMENT)
        } else {
            value.clone()
        };

        let timestamp = chrono::Utc::now().to_rfc3339();
        let event_data = json!({
            "key": key,
            "old_value": redacted_old,
            "new_value": redacted_new,
        });

        emit_event(
            &self.emitter,
            "apcore.config.updated",
            "system.control.update_config",
            &timestamp,
            event_data,
        )
        .await;

        if sensitive {
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

        let entry = build_audit_entry(
            AuditAction::UpdateConfig,
            "system.control.update_config",
            ctx,
            AuditChange {
                before: redacted_old.clone(),
                after: redacted_new.clone(),
            },
        );
        record_audit(self.audit_store.as_ref(), entry).await;

        Ok(json!({
            "success": true,
            "key": key,
            "old_value": redacted_old,
            "new_value": redacted_new,
        }))
    }
}

// ---------------------------------------------------------------------------
// ReloadModule (F10) — single + bulk path_filter reload
// ---------------------------------------------------------------------------

/// Hot-reload a module via safe unregister (F10).
///
/// Full re-discovery is not supported in Rust (no dynamic loading); the
/// module is unregistered and callers must re-register manually. The reload
/// event is always emitted with `new_version` == `previous_version`.
///
/// When `path_filter` is supplied instead of `module_id`, every module ID
/// matching the glob pattern is reloaded in dependency-topological order
/// (leaves first). Supplying both inputs raises `MODULE_RELOAD_CONFLICT`.
pub struct ReloadModule {
    registry: Arc<Registry>,
    emitter: Arc<Mutex<EventEmitter>>,
    audit_store: Option<Arc<dyn AuditStore>>,
}

impl ReloadModule {
    pub fn new(registry: Arc<Registry>, emitter: Arc<Mutex<EventEmitter>>) -> Self {
        Self {
            registry,
            emitter,
            audit_store: None,
        }
    }

    #[must_use]
    pub fn with_audit_store(mut self, audit_store: Option<Arc<dyn AuditStore>>) -> Self {
        self.audit_store = audit_store;
        self
    }

    /// Topologically sort the matched module IDs (leaves first). Falls back
    /// to alphabetical order if the dependency graph contains a cycle or
    /// references a missing module — the reload still happens, just without
    /// the optimal order.
    fn topo_sort_modules(&self, matched: &[String]) -> Vec<String> {
        let matched_set: std::collections::HashSet<String> = matched.iter().cloned().collect();
        let entries: Vec<(String, Vec<DepInfo>)> = matched
            .iter()
            .map(|mid| {
                let deps: Vec<DepInfo> = self
                    .registry
                    .get_definition(mid)
                    .map(|d| {
                        d.dependencies
                            .into_iter()
                            .filter(|dep| matched_set.contains(&dep.module_id))
                            .map(|dep| DepInfo {
                                module_id: dep.module_id,
                                version: if dep.version_constraint.is_empty() {
                                    None
                                } else {
                                    Some(dep.version_constraint)
                                },
                                optional: dep.optional,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                (mid.clone(), deps)
            })
            .collect();

        match resolve_dependencies(&entries, Some(&matched_set), None) {
            Ok(order) => order,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Topological sort failed for path_filter reload; falling back to alphabetical"
                );
                let mut sorted = matched.to_vec();
                sorted.sort();
                sorted
            }
        }
    }

    async fn execute_single(
        &self,
        module_id: String,
        reason: &str,
        ctx: &Context<serde_json::Value>,
    ) -> Result<serde_json::Value, ModuleError> {
        let start = std::time::Instant::now();

        if !self.registry.has(&module_id) {
            return Err(ModuleError::new(
                ErrorCode::ModuleNotFound,
                format!("Module '{module_id}' not found"),
            ));
        }
        // W-1: version is not tracked in the Rust registry descriptor; "unknown"
        // matches the placeholder used pre-hardening.
        self.registry.safe_unregister(&module_id, 5000).await?;
        let previous_version = "unknown".to_string();

        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        let new_version = previous_version.clone();
        let timestamp = chrono::Utc::now().to_rfc3339();
        emit_event(
            &self.emitter,
            "apcore.module.reloaded",
            &module_id,
            &timestamp,
            json!({
                "previous_version": previous_version,
                "new_version": new_version,
            }),
        )
        .await;

        tracing::info!(
            module_id = %module_id,
            previous_version = %previous_version,
            new_version = %new_version,
            reason = %reason,
            "Module reloaded"
        );

        let entry = build_audit_entry(
            AuditAction::ReloadModule,
            &module_id,
            ctx,
            AuditChange {
                before: json!(previous_version),
                after: json!(new_version),
            },
        );
        record_audit(self.audit_store.as_ref(), entry).await;

        Ok(json!({
            "success": true,
            "module_id": module_id,
            "previous_version": previous_version,
            "new_version": new_version,
            "reload_duration_ms": elapsed_ms,
        }))
    }

    async fn execute_bulk(
        &self,
        path_filter: String,
        reason: &str,
        ctx: &Context<serde_json::Value>,
    ) -> Result<serde_json::Value, ModuleError> {
        let pattern = Pattern::new(&path_filter).map_err(|e| {
            ModuleError::new(
                ErrorCode::GeneralInvalidInput,
                format!("'path_filter' is not a valid glob pattern: {e}"),
            )
        })?;

        let mut matched: Vec<String> = self
            .registry
            .module_ids()
            .into_iter()
            .filter(|id| pattern.matches(id))
            .collect();
        matched.sort();

        let order = self.topo_sort_modules(&matched);
        let start = std::time::Instant::now();

        let mut reloaded: Vec<String> = Vec::new();
        for mid in order {
            if !self.registry.has(&mid) {
                continue;
            }
            match self.registry.safe_unregister(&mid, 5000).await {
                Ok(_) => {
                    let timestamp = chrono::Utc::now().to_rfc3339();
                    emit_event(
                        &self.emitter,
                        "apcore.module.reloaded",
                        &mid,
                        &timestamp,
                        json!({"previous_version": "unknown", "new_version": "unknown"}),
                    )
                    .await;
                    reloaded.push(mid);
                }
                Err(e) => {
                    tracing::error!(error = %e, module_id = %mid, "Bulk reload: failed to unregister");
                }
            }
        }

        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        tracing::info!(
            count = reloaded.len(),
            path_filter = %path_filter,
            reason = %reason,
            "Bulk module reload"
        );

        let entry = build_audit_entry(
            AuditAction::ReloadModule,
            &path_filter,
            ctx,
            AuditChange {
                before: serde_json::Value::Null,
                after: json!(reloaded.clone()),
            },
        );
        record_audit(self.audit_store.as_ref(), entry).await;

        Ok(json!({
            "success": true,
            "module_id": serde_json::Value::Null,
            "reloaded_modules": reloaded,
            "reload_duration_ms": elapsed_ms,
        }))
    }
}

#[async_trait]
impl Module for ReloadModule {
    fn description(&self) -> &'static str {
        "Hot-reload a module by safe unregister (re-registration must be done explicitly in Rust)"
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["reason"],
            "properties": {
                "module_id":         {"type": "string"},
                "path_filter":       {"type": "string"},
                "reload_dependents": {"type": "boolean", "default": false},
                "reason":            {"type": "string"}
            }
        })
    }

    fn output_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "success":            {"type": "boolean"},
                "module_id":          {"type": ["string", "null"]},
                "previous_version":   {"type": "string"},
                "new_version":        {"type": "string"},
                "reload_duration_ms": {"type": "number"},
                "reloaded_modules":   {"type": "array", "items": {"type": "string"}}
            }
        })
    }

    async fn execute(
        &self,
        inputs: serde_json::Value,
        ctx: &Context<serde_json::Value>,
    ) -> Result<serde_json::Value, ModuleError> {
        let reason = require_string(&inputs, "reason")?;

        let module_id_input = inputs
            .get("module_id")
            .filter(|v| !v.is_null())
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());
        let path_filter_input = inputs
            .get("path_filter")
            .filter(|v| !v.is_null())
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());

        if module_id_input.is_some() && path_filter_input.is_some() {
            return Err(ModuleError::new(
                ErrorCode::ModuleReloadConflict,
                "'module_id' and 'path_filter' are mutually exclusive",
            ));
        }

        if let Some(filter) = path_filter_input {
            return self.execute_bulk(filter.to_string(), &reason, ctx).await;
        }

        let module_id = module_id_input.ok_or_else(|| {
            ModuleError::new(
                ErrorCode::GeneralInvalidInput,
                "'module_id' or 'path_filter' is required",
            )
        })?;

        self.execute_single(module_id.to_string(), &reason, ctx)
            .await
    }
}

// ---------------------------------------------------------------------------
// ToggleFeatureModule (F19) — runtime enable/disable with optional persistence
// ---------------------------------------------------------------------------

/// Disable or enable a module without unloading it from the Registry (F19).
pub struct ToggleFeatureModule {
    registry: Arc<Registry>,
    emitter: Arc<Mutex<EventEmitter>>,
    toggle_state: Arc<ToggleState>,
    overrides_path: Option<PathBuf>,
    audit_store: Option<Arc<dyn AuditStore>>,
}

impl ToggleFeatureModule {
    pub fn new(
        registry: Arc<Registry>,
        emitter: Arc<Mutex<EventEmitter>>,
        toggle_state: Arc<ToggleState>,
    ) -> Self {
        Self {
            registry,
            emitter,
            toggle_state,
            overrides_path: None,
            audit_store: None,
        }
    }

    #[must_use]
    pub fn with_overrides_path(mut self, overrides_path: Option<PathBuf>) -> Self {
        self.overrides_path = overrides_path;
        self
    }

    #[must_use]
    pub fn with_audit_store(mut self, audit_store: Option<Arc<dyn AuditStore>>) -> Self {
        self.audit_store = audit_store;
        self
    }
}

#[async_trait]
impl Module for ToggleFeatureModule {
    fn description(&self) -> &'static str {
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
        ctx: &Context<serde_json::Value>,
    ) -> Result<serde_json::Value, ModuleError> {
        let module_id = require_string(&inputs, "module_id")?;
        let reason = require_string(&inputs, "reason")?;
        let enabled = inputs
            .get("enabled")
            .and_then(serde_json::Value::as_bool)
            .ok_or_else(|| {
                ModuleError::new(
                    ErrorCode::GeneralInvalidInput,
                    "'enabled' is required and must be a boolean",
                )
            })?;

        if !self.registry.has(&module_id) {
            return Err(ModuleError::new(
                ErrorCode::ModuleNotFound,
                format!("Module '{module_id}' not found"),
            ));
        }

        let before_enabled = !self.toggle_state.is_disabled(&module_id);

        // Flip the descriptor's `enabled` flag in the Registry first — that's
        // the fallible operation. Only after it succeeds do we update the
        // infallible `ToggleState`. This ordering guarantees the two stores
        // cannot diverge on Registry rejection.
        if enabled {
            self.registry.enable(&module_id)?;
            self.toggle_state.enable(&module_id);
        } else {
            self.registry.disable(&module_id)?;
            self.toggle_state.disable(&module_id);
        }

        if let Some(path) = self.overrides_path.as_deref() {
            write_override(
                path,
                &format!("toggle.{module_id}"),
                &serde_json::Value::Bool(enabled),
            );
        }

        let timestamp = chrono::Utc::now().to_rfc3339();
        emit_event(
            &self.emitter,
            "apcore.module.toggled",
            &module_id,
            &timestamp,
            json!({"enabled": enabled}),
        )
        .await;

        tracing::info!(
            module_id = %module_id,
            enabled = %enabled,
            reason = %reason,
            "Module toggled"
        );

        let entry = build_audit_entry(
            AuditAction::ToggleFeature,
            &module_id,
            ctx,
            AuditChange {
                before: json!(before_enabled),
                after: json!(enabled),
            },
        );
        record_audit(self.audit_store.as_ref(), entry).await;

        Ok(json!({
            "success": true,
            "module_id": module_id,
            "enabled": enabled,
        }))
    }
}

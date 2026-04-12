// APCore Protocol — System control modules
// Spec reference: system.control.update_config (F11), system.control.reload_module (F10),
//                 system.control.toggle_feature (F19)

use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::config::Config;
use crate::context::Context;
use crate::errors::{ErrorCode, ModuleError};
use crate::events::emitter::EventEmitter;
use crate::module::Module;
use crate::registry::registry::Registry;

use super::{
    emit_event, is_sensitive_key, missing_field_error, require_string, ToggleState, RESTRICTED_KEYS,
};

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
    registry: Arc<Registry>,
    emitter: Arc<Mutex<EventEmitter>>,
}

impl ReloadModuleModule {
    pub fn new(registry: Arc<Registry>, emitter: Arc<Mutex<EventEmitter>>) -> Self {
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

        // W-1: Version is not tracked in the Rust registry descriptor; use "unknown".
        if !self.registry.has(&module_id) {
            return Err(ModuleError::new(
                ErrorCode::ModuleNotFound,
                format!("Module '{}' not found", module_id),
            ));
        }
        self.registry.safe_unregister(&module_id, 5000).await?;
        let previous_version = "unknown".to_string();

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
    registry: Arc<Registry>,
    emitter: Arc<Mutex<EventEmitter>>,
    toggle_state: Arc<ToggleState>,
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

        if !self.registry.has(&module_id) {
            return Err(ModuleError::new(
                ErrorCode::ModuleNotFound,
                format!("Module '{}' not found", module_id),
            ));
        }

        // Flip the descriptor's `enabled` flag in the Registry first — this is
        // the fallible operation (it may return `ModuleNotFound`). Only after
        // it succeeds do we update the infallible `ToggleState`. Doing it in
        // this order guarantees the two stores cannot diverge if Registry
        // rejects the update.
        if enabled {
            self.registry.enable(&module_id)?;
            self.toggle_state.enable(&module_id);
        } else {
            self.registry.disable(&module_id)?;
            self.toggle_state.disable(&module_id);
        }

        let timestamp = chrono::Utc::now().to_rfc3339();
        let event_data = json!({"enabled": enabled});

        emit_event(
            &self.emitter,
            "apcore.module.toggled",
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

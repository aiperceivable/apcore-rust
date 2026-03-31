// APCore Protocol — System manifest modules
// Spec reference: system.manifest.module, system.manifest.full

use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::config::Config;
use crate::context::Context;
use crate::errors::{ErrorCode, ModuleError};
use crate::module::Module;
use crate::registry::registry::Registry;

/// system.manifest.module — Full manifest for a single registered module.
pub struct ManifestModuleModule {
    registry: Arc<Mutex<Registry>>,
    config: Arc<Mutex<Config>>,
}

impl ManifestModuleModule {
    pub fn new(registry: Arc<Mutex<Registry>>, config: Arc<Mutex<Config>>) -> Self {
        Self { registry, config }
    }
}

#[async_trait]
impl Module for ManifestModuleModule {
    fn description(&self) -> &str {
        "Full manifest for a single registered module"
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["module_id"],
            "properties": {
                "module_id": {"type": "string"}
            }
        })
    }

    fn output_schema(&self) -> serde_json::Value {
        json!({ "type": "object" })
    }

    async fn execute(
        &self,
        inputs: serde_json::Value,
        _ctx: &Context<serde_json::Value>,
    ) -> Result<serde_json::Value, ModuleError> {
        let module_id = inputs
            .get("module_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ModuleError::new(ErrorCode::GeneralInvalidInput, "'module_id' is required")
            })?;

        let reg = self.registry.lock().await;
        let descriptor = reg.get_definition(module_id).ok_or_else(|| {
            ModuleError::new(
                ErrorCode::ModuleNotFound,
                format!("Module '{}' not found", module_id),
            )
        })?;

        let source_root = {
            let cfg = self.config.lock().await;
            cfg.get("project.source_root")
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_default()
        };

        let source_path = if source_root.is_empty() {
            module_id.replace('.', "/") + ".rs"
        } else {
            format!("{}/{}.rs", source_root, module_id.replace('.', "/"))
        };

        let module_ref = reg.get(module_id);
        let description = module_ref
            .map(|m| m.description().to_string())
            .unwrap_or_default();

        // Module trait doesn't expose documentation or metadata, so use defaults.
        let documentation = serde_json::Value::Null;
        let metadata = json!({});

        Ok(json!({
            "module_id": module_id,
            "description": description,
            "documentation": documentation,
            "source_path": source_path,
            "input_schema": descriptor.input_schema,
            "output_schema": descriptor.output_schema,
            "annotations": {
                "readonly": descriptor.annotations.readonly,
                "idempotent": descriptor.annotations.idempotent,
                "requires_approval": descriptor.annotations.requires_approval,
                "destructive": descriptor.annotations.destructive,
            },
            "tags": descriptor.tags,
            "dependencies": descriptor.dependencies,
            "metadata": metadata,
        }))
    }
}

/// system.manifest.full — Complete system manifest with filtering.
pub struct ManifestFullModule {
    registry: Arc<Mutex<Registry>>,
    config: Arc<Mutex<Config>>,
}

impl ManifestFullModule {
    pub fn new(registry: Arc<Mutex<Registry>>, config: Arc<Mutex<Config>>) -> Self {
        Self { registry, config }
    }
}

#[async_trait]
impl Module for ManifestFullModule {
    fn description(&self) -> &str {
        "Complete system manifest with filtering"
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "include_schemas": {"type": "boolean", "default": true},
                "include_source_paths": {"type": "boolean", "default": true},
                "prefix": {"type": "string"},
                "tags": {"type": "array", "items": {"type": "string"}}
            }
        })
    }

    fn output_schema(&self) -> serde_json::Value {
        json!({ "type": "object" })
    }

    async fn execute(
        &self,
        inputs: serde_json::Value,
        _ctx: &Context<serde_json::Value>,
    ) -> Result<serde_json::Value, ModuleError> {
        let include_schemas = inputs
            .get("include_schemas")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let include_source_paths = inputs
            .get("include_source_paths")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let prefix = inputs.get("prefix").and_then(|v| v.as_str());
        let filter_tags: Option<Vec<&str>> = inputs
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect());

        let (project_name, source_root) = {
            let cfg = self.config.lock().await;
            let name = cfg
                .get("project.name")
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| "apcore".to_string());
            let root = cfg
                .get("project.source_root")
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_default();
            (name, root)
        };

        let reg = self.registry.lock().await;
        let all_ids = reg.list(None, None);

        let mut modules = Vec::new();
        for mid in &all_ids {
            // Prefix filter.
            if let Some(pfx) = prefix {
                if !mid.starts_with(pfx) {
                    continue;
                }
            }
            let descriptor = match reg.get_definition(mid) {
                Some(d) => d,
                None => continue,
            };
            // Tag filter (all tags must match).
            if let Some(ref tags) = filter_tags {
                if !tags
                    .iter()
                    .all(|t| descriptor.tags.iter().any(|dt| dt == t))
                {
                    continue;
                }
            }

            let module_ref = reg.get(mid);
            let description = module_ref
                .map(|m| m.description().to_string())
                .unwrap_or_default();

            let mut entry = json!({
                "module_id": mid,
                "description": description,
                "annotations": {
                    "readonly": descriptor.annotations.readonly,
                    "idempotent": descriptor.annotations.idempotent,
                    "requires_approval": descriptor.annotations.requires_approval,
                    "destructive": descriptor.annotations.destructive,
                },
                "tags": descriptor.tags,
                "dependencies": descriptor.dependencies,
            });

            if include_schemas {
                entry["input_schema"] = descriptor.input_schema.clone();
                entry["output_schema"] = descriptor.output_schema.clone();
            }
            if include_source_paths {
                let sp = if source_root.is_empty() {
                    mid.replace('.', "/") + ".rs"
                } else {
                    format!("{}/{}.rs", source_root, mid.replace('.', "/"))
                };
                entry["source_path"] = json!(sp);
            }

            modules.push(entry);
        }

        Ok(json!({
            "project_name": project_name,
            "module_count": modules.len(),
            "modules": modules,
        }))
    }
}

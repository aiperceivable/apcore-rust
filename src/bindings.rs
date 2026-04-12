// APCore Protocol — Binding loader
// Spec reference: Module binding resolution from external sources

use serde::{Deserialize, Serialize};
use serde_yaml_ng as serde_yaml;
use std::collections::HashMap;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use crate::context::Context;
use crate::decorator::FunctionModule;
use crate::errors::ModuleError;
use crate::module::ModuleAnnotations;
use crate::registry::registry::Registry;

/// Boxed async handler type used by [`BindingLoader::register_into_with_handlers`].
///
/// The handler takes the module inputs plus a reference to the execution
/// context and returns a JSON result. Handlers are stored as `Arc` so they
/// can be cheaply cloned when materializing multiple modules.
pub type BindingHandler = Arc<
    dyn for<'a> Fn(
            serde_json::Value,
            &'a Context<serde_json::Value>,
        ) -> Pin<
            Box<
                dyn std::future::Future<Output = Result<serde_json::Value, ModuleError>>
                    + Send
                    + 'a,
            >,
        > + Send
        + Sync,
>;

/// Describes a binding target.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BindingTarget {
    pub module_name: String,
    pub callable: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema_path: Option<String>,
}

/// A resolved binding definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BindingDefinition {
    pub name: String,
    pub target: BindingTarget,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Loads and resolves module bindings from files or configuration.
#[derive(Debug)]
pub struct BindingLoader {
    bindings: HashMap<String, BindingDefinition>,
}

impl BindingLoader {
    /// Create a new empty binding loader.
    pub fn new() -> Self {
        Self {
            bindings: HashMap::new(),
        }
    }

    /// Load bindings from a JSON file.
    pub fn load_from_file(&mut self, path: &Path) -> Result<(), ModuleError> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            ModuleError::new(
                crate::errors::ErrorCode::BindingFileInvalid,
                format!("Failed to read binding file '{}': {}", path.display(), e),
            )
        })?;

        let definitions: Vec<BindingDefinition> = serde_json::from_str(&content).map_err(|e| {
            ModuleError::new(
                crate::errors::ErrorCode::BindingFileInvalid,
                format!("Failed to parse binding file '{}': {}", path.display(), e),
            )
        })?;

        for def in definitions {
            self.bindings.insert(def.name.clone(), def);
        }

        Ok(())
    }

    /// Load bindings from a YAML file.
    pub fn load_from_yaml(&mut self, path: &Path) -> Result<(), ModuleError> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            ModuleError::new(
                crate::errors::ErrorCode::BindingFileInvalid,
                format!(
                    "Failed to read binding YAML file '{}': {}",
                    path.display(),
                    e
                ),
            )
        })?;

        let definitions: Vec<BindingDefinition> = serde_yaml::from_str(&content).map_err(|e| {
            ModuleError::new(
                crate::errors::ErrorCode::BindingFileInvalid,
                format!(
                    "Failed to parse binding YAML file '{}': {}",
                    path.display(),
                    e
                ),
            )
        })?;

        for def in definitions {
            self.bindings.insert(def.name.clone(), def);
        }

        Ok(())
    }

    /// Resolve a binding by name.
    pub fn resolve(&self, name: &str) -> Result<&BindingDefinition, ModuleError> {
        self.bindings.get(name).ok_or_else(|| {
            ModuleError::new(
                crate::errors::ErrorCode::BindingModuleNotFound,
                format!("Binding '{}' not found", name),
            )
        })
    }

    /// List all loaded binding names.
    pub fn list_bindings(&self) -> Vec<&str> {
        self.bindings.keys().map(|k| k.as_str()).collect()
    }

    /// Register every loaded binding as a [`FunctionModule`] in `registry`,
    /// resolving each binding's executable handler from `handlers`.
    ///
    /// Python resolves `BindingTarget::callable` via dynamic import; Rust
    /// cannot do that, so the caller supplies a map from binding name to
    /// handler closure. Each binding is registered under its `name` and the
    /// method returns the number of modules registered.
    ///
    /// Returns an error if any binding is missing a handler or if the
    /// underlying [`Registry::register_module`] call fails.
    pub fn register_into_with_handlers(
        &self,
        registry: &Registry,
        handlers: HashMap<String, BindingHandler>,
    ) -> Result<usize, ModuleError> {
        let mut count = 0usize;
        for (name, def) in &self.bindings {
            let handler = handlers.get(name).cloned().ok_or_else(|| {
                ModuleError::new(
                    crate::errors::ErrorCode::BindingModuleNotFound,
                    format!("No handler provided for binding '{}'", name),
                )
            })?;

            let description = def
                .metadata
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let module = FunctionModule::with_description(
                ModuleAnnotations::default(),
                serde_json::json!({"type": "object"}),
                serde_json::json!({"type": "object"}),
                description,
                None,
                Vec::new(),
                "0.1.0",
                HashMap::new(),
                Vec::new(),
                move |inputs, ctx| {
                    let handler = Arc::clone(&handler);
                    Box::pin(async move { (handler)(inputs, ctx).await })
                },
            );

            registry.register_module(def.target.module_name.as_str(), Box::new(module))?;
            count += 1;
        }
        Ok(count)
    }
}

impl Default for BindingLoader {
    fn default() -> Self {
        Self::new()
    }
}

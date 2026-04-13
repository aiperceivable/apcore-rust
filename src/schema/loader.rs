// APCore Protocol — Schema loader
// Spec reference: Loading schemas from files and inline definitions

use serde_yaml_ng as serde_yaml;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::errors::{ErrorCode, ModuleError};
use crate::schema::SchemaDefinition;

/// Strategy for loading schemas when both YAML and native definitions exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaStrategy {
    /// Prefer YAML schema files over native code definitions.
    YamlFirst,
    /// Prefer native code definitions over YAML files.
    NativeFirst,
    /// Use only YAML files; ignore native definitions.
    YamlOnly,
}

/// Loads JSON schemas from various sources.
#[derive(Debug)]
pub struct SchemaLoader {
    schemas: HashMap<String, serde_json::Value>,
    pub strategy: SchemaStrategy,
    /// Optional base directory used by the spec-compatible `load()` method.
    schemas_dir: Option<PathBuf>,
}

impl SchemaLoader {
    /// Create a new schema loader with the default strategy.
    pub fn new() -> Self {
        Self {
            schemas: HashMap::new(),
            strategy: SchemaStrategy::YamlFirst,
            schemas_dir: None,
        }
    }

    /// Create a schema loader with a specific strategy.
    pub fn with_strategy(strategy: SchemaStrategy) -> Self {
        Self {
            schemas: HashMap::new(),
            strategy,
            schemas_dir: None,
        }
    }

    /// Spec-compatible constructor: create a loader from a `Config` and optional schemas directory.
    ///
    /// `schemas_dir` overrides the directory used by [`Self::load`] to resolve schema files.
    /// When `schemas_dir` is `None` the loader falls back to `config.modules_path` and
    /// finally to the current working directory.
    pub fn with_config(config: &Config, schemas_dir: Option<&Path>) -> Self {
        let resolved_dir = schemas_dir
            .map(|p| p.to_path_buf())
            .or_else(|| config.modules_path.clone());
        Self {
            schemas: HashMap::new(),
            strategy: SchemaStrategy::YamlFirst,
            schemas_dir: resolved_dir,
        }
    }

    /// Spec-compatible load: resolve a schema for `module_id` and return a [`SchemaDefinition`].
    ///
    /// Resolution order:
    /// 1. If the schema was previously loaded in-memory via [`Self::load_from_value`] or
    ///    [`Self::load_from_file`], return it wrapped in a `SchemaDefinition`.
    /// 2. Otherwise, attempt to load `<schemas_dir>/<module_id>.json` (then `.yaml`).
    ///
    /// `schemas_dir` is the directory supplied to [`Self::with_config`], or the current working
    /// directory when none was provided.
    pub fn load(&mut self, module_id: &str) -> Result<SchemaDefinition, ModuleError> {
        // 1. Already in memory?
        if let Some(value) = self.get(module_id) {
            return Self::value_to_schema_def(module_id, value.clone());
        }

        // 2. Try to find a file on disk.
        let base = self
            .schemas_dir
            .clone()
            .unwrap_or_else(|| PathBuf::from("."));

        // Try .json first, then .yaml/.yml
        let candidates = [
            base.join(format!("{module_id}.json")),
            base.join(format!("{module_id}.yaml")),
            base.join(format!("{module_id}.yml")),
        ];

        let mut last_err: Option<ModuleError> = None;
        for path in &candidates {
            if path.exists() {
                self.load_from_file(module_id, path)?;
                let value = self.get(module_id).expect("just loaded").clone();
                return Self::value_to_schema_def(module_id, value);
            } else {
                last_err = Some(ModuleError::new(
                    ErrorCode::SchemaNotFound,
                    format!(
                        "Schema file not found for module '{}' (tried {})",
                        module_id,
                        path.display()
                    ),
                ));
            }
        }

        Err(last_err.unwrap_or_else(|| {
            ModuleError::new(
                ErrorCode::SchemaNotFound,
                format!("Schema not found for module '{module_id}'"),
            )
        }))
    }

    /// Convert a raw JSON `Value` into a [`SchemaDefinition`] for `module_id`.
    fn value_to_schema_def(
        module_id: &str,
        value: serde_json::Value,
    ) -> Result<SchemaDefinition, ModuleError> {
        // If the stored value already has the SchemaDefinition shape, deserialize it.
        if value.get("input_schema").is_some() && value.get("output_schema").is_some() {
            return serde_json::from_value::<SchemaDefinition>(value).map_err(|e| {
                ModuleError::new(
                    ErrorCode::SchemaParseError,
                    format!("Failed to deserialize SchemaDefinition for '{module_id}': {e}"),
                )
            });
        }

        // Otherwise, treat the value as the input_schema itself and build a minimal definition.
        Ok(SchemaDefinition {
            module_id: module_id.to_string(),
            description: value
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            input_schema: value
                .get("input_schema")
                .or_else(|| value.get("inputSchema"))
                .cloned()
                .unwrap_or_else(|| value.clone()),
            output_schema: value
                .get("output_schema")
                .or_else(|| value.get("outputSchema"))
                .cloned()
                .unwrap_or(serde_json::json!({})),
            error_schema: None,
            definitions: None,
            version: None,
        })
    }

    /// Load a schema from a JSON/YAML file.
    pub fn load_from_file(&mut self, name: &str, path: &Path) -> Result<(), ModuleError> {
        let contents = std::fs::read_to_string(path).map_err(|e| {
            ModuleError::new(
                ErrorCode::SchemaNotFound,
                format!("Failed to read schema file '{}': {}", path.display(), e),
            )
        })?;

        // Determine format from extension; default to YAML (which also handles JSON).
        let value: serde_json::Value = if path.extension().is_some_and(|ext| ext == "json") {
            serde_json::from_str(&contents).map_err(|e| {
                ModuleError::new(
                    ErrorCode::SchemaParseError,
                    format!("Failed to parse JSON schema '{}': {}", path.display(), e),
                )
            })?
        } else {
            // YAML parser handles both .yaml/.yml (and is a superset of JSON)
            serde_yaml::from_str(&contents).map_err(|e| {
                ModuleError::new(
                    ErrorCode::SchemaParseError,
                    format!("Failed to parse YAML schema '{}': {}", path.display(), e),
                )
            })?
        };

        self.schemas.insert(name.to_string(), value);
        Ok(())
    }

    /// Load a schema from a JSON value.
    pub fn load_from_value(
        &mut self,
        name: &str,
        schema: serde_json::Value,
    ) -> Result<(), ModuleError> {
        self.schemas.insert(name.to_string(), schema);
        Ok(())
    }

    /// Get a loaded schema by name.
    pub fn get(&self, name: &str) -> Option<&serde_json::Value> {
        self.schemas.get(name)
    }

    /// List all loaded schema names.
    pub fn list(&self) -> Vec<&str> {
        self.schemas
            .keys()
            .map(std::string::String::as_str)
            .collect()
    }
}

impl Default for SchemaLoader {
    fn default() -> Self {
        Self::new()
    }
}

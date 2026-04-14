// APCore Protocol — Schema exporter
// Spec reference: Exporting schemas in various formats

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::errors::{ErrorCode, ModuleError};

/// Optional parameters for the spec-compatible [`SchemaExporter::export`] method.
#[derive(Debug, Clone, Default)]
pub struct ExportOptions {
    /// Optional annotations to merge into the exported schema (e.g. `x-*` fields).
    pub annotations: Option<serde_json::Value>,
    /// Optional examples array to include in the exported schema.
    pub examples: Option<serde_json::Value>,
    /// Override the schema name in the exported output.
    pub name: Option<String>,
}

/// Profile controlling how schemas are exported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportProfile {
    Mcp,
    #[serde(rename = "openai")]
    OpenAi,
    Anthropic,
    Generic,
}

/// Exports schemas to various formats.
#[derive(Debug)]
pub struct SchemaExporter;

impl SchemaExporter {
    /// Create a new schema exporter.
    pub fn new() -> Self {
        Self
    }

    /// Export a schema using the given profile.
    ///
    /// Returns a [`serde_json::Value`] (spec-compatible with Python/TypeScript `-> dict`).
    /// Optional `options` allows overriding the name and merging annotations/examples.
    pub fn export(
        &self,
        schema: &serde_json::Value,
        profile: ExportProfile,
        options: Option<&ExportOptions>,
    ) -> Result<serde_json::Value, ModuleError> {
        let mut exported = match profile {
            ExportProfile::Mcp => self.export_mcp(schema)?,
            ExportProfile::OpenAi => self.export_openai(schema)?,
            ExportProfile::Anthropic => self.export_anthropic(schema)?,
            ExportProfile::Generic => self.export_generic(schema)?,
        };

        // Apply optional overrides from ExportOptions.
        if let Some(opts) = options {
            if let (Some(obj), Some(name)) = (exported.as_object_mut(), &opts.name) {
                obj.insert("name".to_string(), serde_json::Value::String(name.clone()));
            }
            if let (Some(obj), Some(annotations)) = (exported.as_object_mut(), &opts.annotations) {
                if let Some(ann_obj) = annotations.as_object() {
                    for (k, v) in ann_obj {
                        obj.entry(k.clone()).or_insert_with(|| v.clone());
                    }
                }
            }
            if let (Some(obj), Some(examples)) = (exported.as_object_mut(), &opts.examples) {
                obj.insert("examples".to_string(), examples.clone());
            }
        }

        Ok(exported)
    }

    /// Export a schema to a pretty-printed JSON string (preserves legacy behaviour).
    ///
    /// Equivalent to calling [`Self::export`] and serializing the result.
    pub fn export_serialized(
        &self,
        schema: &serde_json::Value,
        profile: ExportProfile,
        options: Option<&ExportOptions>,
    ) -> Result<String, ModuleError> {
        let value = self.export(schema, profile, options)?;
        serde_json::to_string_pretty(&value).map_err(|e| {
            ModuleError::new(
                ErrorCode::SchemaParseError,
                format!("Failed to serialize exported schema: {e}"),
            )
        })
    }

    /// Export all schemas from a loader using the given profile.
    ///
    /// Returns a map of schema name → [`serde_json::Value`].
    pub fn export_all(
        &self,
        loader: &super::loader::SchemaLoader,
        profile: ExportProfile,
    ) -> Result<HashMap<String, serde_json::Value>, ModuleError> {
        let mut result = HashMap::new();
        for name in loader.list() {
            if let Some(schema) = loader.get(name) {
                let exported = self.export(schema, profile, None)?;
                result.insert(name.to_string(), exported);
            }
        }
        Ok(result)
    }

    /// Export all schemas from a loader to serialized JSON strings (legacy helper).
    pub fn export_all_serialized(
        &self,
        loader: &super::loader::SchemaLoader,
        profile: ExportProfile,
    ) -> Result<HashMap<String, String>, ModuleError> {
        let mut result = HashMap::new();
        for name in loader.list() {
            if let Some(schema) = loader.get(name) {
                let exported = self.export_serialized(schema, profile, None)?;
                result.insert(name.to_string(), exported);
            }
        }
        Ok(result)
    }

    /// MCP format: { name, inputSchema }
    #[allow(clippy::unused_self)] // consistent method signature for dispatch through export()
    #[allow(clippy::unnecessary_wraps)] // consistent Result return for dispatch through export()
    fn export_mcp(&self, schema: &serde_json::Value) -> Result<serde_json::Value, ModuleError> {
        let name = schema
            .get("name")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let input_schema = schema
            .get("input_schema")
            .or_else(|| schema.get("inputSchema"))
            .cloned()
            .unwrap_or(serde_json::json!({}));

        Ok(serde_json::json!({
            "name": name,
            "inputSchema": input_schema,
        }))
    }

    /// OpenAI format: { type: "function", function: { name, description, parameters, strict } }
    #[allow(clippy::unused_self)] // consistent method signature for dispatch through export()
    #[allow(clippy::unnecessary_wraps)] // consistent Result return for dispatch through export()
    fn export_openai(&self, schema: &serde_json::Value) -> Result<serde_json::Value, ModuleError> {
        let name = schema
            .get("name")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let description = schema
            .get("description")
            .cloned()
            .unwrap_or(serde_json::Value::String(String::new()));
        let parameters = schema
            .get("input_schema")
            .or_else(|| schema.get("inputSchema"))
            .or_else(|| schema.get("parameters"))
            .cloned()
            .unwrap_or(serde_json::json!({}));

        Ok(serde_json::json!({
            "type": "function",
            "function": {
                "name": name,
                "description": description,
                "parameters": parameters,
                "strict": true,
            }
        }))
    }

    /// Anthropic format: { name, description, input_schema }
    #[allow(clippy::unused_self)] // consistent method signature for dispatch through export()
    #[allow(clippy::unnecessary_wraps)] // consistent Result return for dispatch through export()
    fn export_anthropic(
        &self,
        schema: &serde_json::Value,
    ) -> Result<serde_json::Value, ModuleError> {
        let name = schema
            .get("name")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let description = schema
            .get("description")
            .cloned()
            .unwrap_or(serde_json::Value::String(String::new()));
        let input_schema = schema
            .get("input_schema")
            .or_else(|| schema.get("inputSchema"))
            .cloned()
            .unwrap_or(serde_json::json!({}));

        Ok(serde_json::json!({
            "name": name,
            "description": description,
            "input_schema": input_schema,
        }))
    }

    /// Generic format: return schema as-is.
    #[allow(clippy::unused_self)] // consistent method signature for dispatch through export()
    #[allow(clippy::unnecessary_wraps)] // consistent Result return for dispatch through export()
    fn export_generic(&self, schema: &serde_json::Value) -> Result<serde_json::Value, ModuleError> {
        Ok(schema.clone())
    }
}

impl Default for SchemaExporter {
    fn default() -> Self {
        Self::new()
    }
}

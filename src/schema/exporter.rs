// APCore Protocol — Schema exporter
// Spec reference: Exporting schemas in various formats

use std::collections::HashMap;

use crate::errors::{ErrorCode, ModuleError};

/// Profile controlling how schemas are exported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportProfile {
    Mcp,
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
    pub fn export(
        &self,
        schema: &serde_json::Value,
        profile: ExportProfile,
    ) -> Result<String, ModuleError> {
        let exported = match profile {
            ExportProfile::Mcp => self.export_mcp(schema)?,
            ExportProfile::OpenAi => self.export_openai(schema)?,
            ExportProfile::Anthropic => self.export_anthropic(schema)?,
            ExportProfile::Generic => self.export_generic(schema)?,
        };
        serde_json::to_string_pretty(&exported).map_err(|e| {
            ModuleError::new(
                ErrorCode::SchemaParseError,
                format!("Failed to serialize exported schema: {}", e),
            )
        })
    }

    /// Export all schemas from a loader using the given profile.
    pub fn export_all(
        &self,
        loader: &super::loader::SchemaLoader,
        profile: ExportProfile,
    ) -> Result<HashMap<String, String>, ModuleError> {
        let mut result = HashMap::new();
        for name in loader.list() {
            if let Some(schema) = loader.get(name) {
                let exported = self.export(schema, profile)?;
                result.insert(name.to_string(), exported);
            }
        }
        Ok(result)
    }

    /// MCP format: { name, inputSchema }
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
    fn export_generic(&self, schema: &serde_json::Value) -> Result<serde_json::Value, ModuleError> {
        Ok(schema.clone())
    }
}

impl Default for SchemaExporter {
    fn default() -> Self {
        Self::new()
    }
}

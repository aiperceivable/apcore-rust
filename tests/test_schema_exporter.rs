//! Tests for SchemaExporter — exporting schemas to MCP, OpenAI, Anthropic, and Generic formats.
#![allow(clippy::similar_names)] // `exporter`/`exported` and `schema`/`schemas` are intentionally distinct

use apcore::schema::{ExportProfile, SchemaExporter, SchemaLoader};
use serde_json::json;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn sample_schema() -> serde_json::Value {
    json!({
        "name": "get_weather",
        "description": "Get the current weather for a location",
        "input_schema": {
            "type": "object",
            "properties": {
                "location": { "type": "string" }
            },
            "required": ["location"]
        }
    })
}

// ---------------------------------------------------------------------------
// MCP export
// ---------------------------------------------------------------------------

#[test]
fn test_schema_exporter_mcp_format() {
    let exporter = SchemaExporter::new();
    let exported = exporter
        .export(&sample_schema(), ExportProfile::Mcp)
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&exported).unwrap();
    assert_eq!(parsed["name"], "get_weather");
    assert!(parsed["inputSchema"].is_object());
    assert_eq!(parsed["inputSchema"]["required"][0], "location");
    // MCP format should NOT include description at top level
    assert!(parsed.get("description").is_none());
}

#[test]
fn test_schema_exporter_mcp_missing_name() {
    let exporter = SchemaExporter::new();
    let schema = json!({ "input_schema": { "type": "object" } });
    let exported = exporter.export(&schema, ExportProfile::Mcp).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&exported).unwrap();
    assert!(parsed["name"].is_null());
}

#[test]
fn test_schema_exporter_mcp_fallback_input_schema_key() {
    let exporter = SchemaExporter::new();
    let schema = json!({
        "name": "tool",
        "inputSchema": { "type": "string" }
    });
    let exported = exporter.export(&schema, ExportProfile::Mcp).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&exported).unwrap();
    assert_eq!(parsed["inputSchema"]["type"], "string");
}

#[test]
fn test_schema_exporter_mcp_no_input_schema_defaults_empty_object() {
    let exporter = SchemaExporter::new();
    let schema = json!({ "name": "tool" });
    let exported = exporter.export(&schema, ExportProfile::Mcp).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&exported).unwrap();
    assert_eq!(parsed["inputSchema"], json!({}));
}

// ---------------------------------------------------------------------------
// OpenAI export
// ---------------------------------------------------------------------------

#[test]
fn test_schema_exporter_openai_format() {
    let exporter = SchemaExporter::new();
    let exported = exporter
        .export(&sample_schema(), ExportProfile::OpenAi)
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&exported).unwrap();
    assert_eq!(parsed["type"], "function");
    assert_eq!(parsed["function"]["name"], "get_weather");
    assert_eq!(
        parsed["function"]["description"],
        "Get the current weather for a location"
    );
    assert!(parsed["function"]["parameters"].is_object());
    assert_eq!(parsed["function"]["strict"], true);
}

#[test]
fn test_schema_exporter_openai_missing_description_defaults_empty() {
    let exporter = SchemaExporter::new();
    let schema = json!({ "name": "tool", "input_schema": {} });
    let exported = exporter.export(&schema, ExportProfile::OpenAi).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&exported).unwrap();
    assert_eq!(parsed["function"]["description"], "");
}

#[test]
fn test_schema_exporter_openai_uses_parameters_key_fallback() {
    let exporter = SchemaExporter::new();
    let schema = json!({
        "name": "tool",
        "parameters": { "type": "object", "properties": { "x": { "type": "integer" } } }
    });
    let exported = exporter.export(&schema, ExportProfile::OpenAi).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&exported).unwrap();
    assert_eq!(
        parsed["function"]["parameters"]["properties"]["x"]["type"],
        "integer"
    );
}

// ---------------------------------------------------------------------------
// Anthropic export
// ---------------------------------------------------------------------------

#[test]
fn test_schema_exporter_anthropic_format() {
    let exporter = SchemaExporter::new();
    let exported = exporter
        .export(&sample_schema(), ExportProfile::Anthropic)
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&exported).unwrap();
    assert_eq!(parsed["name"], "get_weather");
    assert_eq!(
        parsed["description"],
        "Get the current weather for a location"
    );
    assert!(parsed["input_schema"].is_object());
    assert_eq!(parsed["input_schema"]["required"][0], "location");
    // Anthropic format should NOT have "type": "function" wrapper
    assert!(parsed.get("type").is_none());
}

#[test]
fn test_schema_exporter_anthropic_missing_fields_use_defaults() {
    let exporter = SchemaExporter::new();
    let schema = json!({});
    let exported = exporter.export(&schema, ExportProfile::Anthropic).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&exported).unwrap();
    assert!(parsed["name"].is_null());
    assert_eq!(parsed["description"], "");
    assert_eq!(parsed["input_schema"], json!({}));
}

// ---------------------------------------------------------------------------
// Generic export
// ---------------------------------------------------------------------------

#[test]
fn test_schema_exporter_generic_returns_schema_as_is() {
    let exporter = SchemaExporter::new();
    let schema = sample_schema();
    let exported = exporter.export(&schema, ExportProfile::Generic).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&exported).unwrap();
    assert_eq!(parsed, schema);
}

#[test]
fn test_schema_exporter_generic_preserves_arbitrary_fields() {
    let exporter = SchemaExporter::new();
    let schema = json!({ "custom_field": 42, "nested": { "a": true } });
    let exported = exporter.export(&schema, ExportProfile::Generic).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&exported).unwrap();
    assert_eq!(parsed["custom_field"], 42);
    assert_eq!(parsed["nested"]["a"], true);
}

// ---------------------------------------------------------------------------
// export_all
// ---------------------------------------------------------------------------

#[test]
fn test_schema_exporter_export_all_empty_loader() {
    let exporter = SchemaExporter::new();
    let loader = SchemaLoader::new();
    let result = exporter
        .export_all(&loader, ExportProfile::Generic)
        .unwrap();
    assert!(result.is_empty());
}

#[test]
fn test_schema_exporter_export_all_multiple_schemas() {
    let exporter = SchemaExporter::new();
    let mut loader = SchemaLoader::new();
    loader
        .load_from_value(
            "tool_a",
            json!({ "name": "tool_a", "input_schema": { "type": "object" } }),
        )
        .unwrap();
    loader
        .load_from_value(
            "tool_b",
            json!({ "name": "tool_b", "input_schema": { "type": "string" } }),
        )
        .unwrap();

    let result = exporter
        .export_all(&loader, ExportProfile::Anthropic)
        .unwrap();
    assert_eq!(result.len(), 2);
    assert!(result.contains_key("tool_a"));
    assert!(result.contains_key("tool_b"));

    // Verify each exported schema is valid JSON
    for exported_str in result.values() {
        let parsed: serde_json::Value = serde_json::from_str(exported_str).unwrap();
        assert!(parsed.get("name").is_some());
    }
}

// ---------------------------------------------------------------------------
// ExportProfile equality
// ---------------------------------------------------------------------------

#[test]
fn test_export_profile_equality() {
    assert_eq!(ExportProfile::Mcp, ExportProfile::Mcp);
    assert_ne!(ExportProfile::Mcp, ExportProfile::OpenAi);
    assert_ne!(ExportProfile::OpenAi, ExportProfile::Anthropic);
    assert_ne!(ExportProfile::Anthropic, ExportProfile::Generic);
}

// ---------------------------------------------------------------------------
// Default impl
// ---------------------------------------------------------------------------

#[test]
fn test_schema_exporter_default() {
    let exporter = SchemaExporter;
    let exported = exporter
        .export(&json!({"name": "x"}), ExportProfile::Generic)
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&exported).unwrap();
    assert_eq!(parsed["name"], "x");
}

// ---------------------------------------------------------------------------
// Output is pretty-printed JSON
// ---------------------------------------------------------------------------

#[test]
fn test_schema_exporter_output_is_pretty_printed() {
    let exporter = SchemaExporter::new();
    let exported = exporter
        .export(&json!({"name": "test"}), ExportProfile::Generic)
        .unwrap();
    // Pretty-printed JSON contains newlines
    assert!(exported.contains('\n'));
}

// APCore Protocol — Module trait and related types
// Spec reference: Module definition, annotations, preflight checks

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::context::Context;
use crate::errors::ModuleError;

/// Core trait that all APCore modules must implement.
#[async_trait]
pub trait Module: Send + Sync {
    /// Returns the JSON Schema describing this module's input.
    fn input_schema(&self) -> serde_json::Value;

    /// Returns the JSON Schema describing this module's output.
    fn output_schema(&self) -> serde_json::Value;

    /// Returns a human-readable description of this module.
    fn description(&self) -> &str;

    /// Execute the module with the given context and input.
    async fn execute(
        &self,
        ctx: &Context<serde_json::Value>,
        input: serde_json::Value,
    ) -> Result<serde_json::Value, ModuleError>;

    /// Run preflight checks before execution.
    fn preflight(&self) -> PreflightResult {
        // TODO: Implement
        PreflightResult {
            passed: true,
            checks: vec![],
        }
    }
}

/// Metadata annotations attached to a module.
/// Describes behavioral characteristics of the module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleAnnotations {
    #[serde(default)]
    pub readonly: bool,
    #[serde(default)]
    pub destructive: bool,
    #[serde(default)]
    pub idempotent: bool,
    #[serde(default)]
    pub requires_approval: bool,
    #[serde(default = "default_true")]
    pub open_world: bool,
    #[serde(default)]
    pub streaming: bool,
    #[serde(default)]
    pub cacheable: bool,
    #[serde(default)]
    pub cache_ttl: u64,
    #[serde(default)]
    pub cache_key_fields: Option<Vec<String>>,
    #[serde(default)]
    pub paginated: bool,
    #[serde(default = "default_pagination_style")]
    pub pagination_style: String, // "cursor" | "offset" | "page"
    // Legacy fields moved to ModuleDescriptor:
    // name, version, author, description, tags, category, deprecated,
    // deprecated_message, since, hidden, examples, dependencies, metadata
}

fn default_true() -> bool { true }
fn default_pagination_style() -> String { "cursor".to_string() }

impl Default for ModuleAnnotations {
    fn default() -> Self {
        Self {
            readonly: false,
            destructive: false,
            idempotent: false,
            requires_approval: false,
            open_world: true,
            streaming: false,
            cacheable: false,
            cache_ttl: 0,
            cache_key_fields: None,
            paginated: false,
            pagination_style: "cursor".to_string(),
        }
    }
}

/// An example input/output pair for documentation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleExample {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub inputs: serde_json::Value,
    pub output: serde_json::Value,
}

/// Result of validating a single aspect.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationResult {
    pub valid: bool,
    #[serde(default)]
    pub errors: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// Result of a single preflight check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreflightCheckResult {
    pub name: String,
    pub passed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Aggregated preflight results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreflightResult {
    pub passed: bool,
    pub checks: Vec<PreflightCheckResult>,
}

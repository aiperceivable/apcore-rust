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

    /// Execute the module with the given inputs and context.
    async fn execute(
        &self,
        inputs: serde_json::Value,
        ctx: &Context<serde_json::Value>,
    ) -> Result<serde_json::Value, ModuleError>;

    /// Return a structured description of this module for AI/LLM consumption (spec §5.6).
    /// Default: builds description from input_schema, output_schema, and description.
    fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "description": self.description(),
            "input_schema": self.input_schema(),
            "output_schema": self.output_schema(),
        })
    }

    /// Run preflight checks before execution.
    fn preflight(&self) -> PreflightResult {
        PreflightResult {
            valid: true,
            checks: vec![],
            requires_approval: false,
        }
    }

    /// Called after the module is registered. Default: no-op.
    fn on_load(&self) {}

    /// Called before the module is unregistered. Default: no-op.
    fn on_unload(&self) {}

    /// Called before hot-reload to capture state. Returns state dict for on_resume().
    /// Default: returns None (no state to preserve).
    fn on_suspend(&self) -> Option<serde_json::Value> {
        None
    }

    /// Called after hot-reload to restore state from on_suspend().
    /// Default: no-op.
    fn on_resume(&self, _state: serde_json::Value) {}
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
    /// Extension map for ecosystem package metadata.
    /// Unknown JSON keys are captured here via serde(flatten).
    #[serde(default, flatten)]
    pub extra: HashMap<String, serde_json::Value>,
    // Legacy fields moved to ModuleDescriptor:
    // name, version, author, description, tags, category, deprecated,
    // deprecated_message, since, hidden, examples, dependencies, metadata
}

fn default_true() -> bool {
    true
}
fn default_pagination_style() -> String {
    "cursor".to_string()
}

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
            extra: HashMap::new(),
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

/// Result of validating a single aspect (used by SchemaValidator and ModuleValidator).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationResult {
    pub valid: bool,
    #[serde(default)]
    pub errors: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// Result of a single preflight check (spec §12.8.4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreflightCheckResult {
    /// Check name (e.g., "module_id", "module_lookup", "call_chain", "acl", "schema", "module_preflight").
    pub check: String,
    /// Whether the check passed.
    pub passed: bool,
    /// Error details when `passed` is false; None when passed is true.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<serde_json::Value>,
    /// Non-fatal advisory messages (default: empty).
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// Aggregated preflight results returned by `Executor::validate()` (spec §12.8.3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreflightResult {
    /// True only if ALL checks passed.
    pub valid: bool,
    /// Ordered list of check results.
    pub checks: Vec<PreflightCheckResult>,
    /// True if the module has `requires_approval` annotation.
    #[serde(default)]
    pub requires_approval: bool,
}

impl PreflightResult {
    /// Computed view: only checks where `passed` is false (duck-type ValidationResult.errors).
    pub fn errors(&self) -> Vec<&PreflightCheckResult> {
        self.checks.iter().filter(|c| !c.passed).collect()
    }
}

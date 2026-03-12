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
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModuleAnnotations {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default)]
    pub deprecated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecated_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub examples: Vec<ModuleExample>,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// An example input/output pair for documentation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleExample {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input: serde_json::Value,
    pub expected_output: serde_json::Value,
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

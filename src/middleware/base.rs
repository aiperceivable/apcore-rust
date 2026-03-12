// APCore Protocol — Middleware base trait
// Spec reference: Middleware lifecycle (before, after, on_error)

use async_trait::async_trait;

use crate::context::Context;
use crate::errors::ModuleError;

/// Core middleware trait with before/after/on_error hooks.
#[async_trait]
pub trait Middleware: Send + Sync + std::fmt::Debug {
    /// Name of this middleware for logging/debugging.
    fn name(&self) -> &str;

    /// Called before module execution. Can modify input.
    async fn before(
        &self,
        ctx: &Context<serde_json::Value>,
        module_name: &str,
        input: serde_json::Value,
    ) -> Result<serde_json::Value, ModuleError>;

    /// Called after successful module execution. Can modify output.
    async fn after(
        &self,
        ctx: &Context<serde_json::Value>,
        module_name: &str,
        output: serde_json::Value,
    ) -> Result<serde_json::Value, ModuleError>;

    /// Called when module execution fails.
    async fn on_error(
        &self,
        ctx: &Context<serde_json::Value>,
        module_name: &str,
        error: &ModuleError,
    ) -> Result<(), ModuleError>;
}

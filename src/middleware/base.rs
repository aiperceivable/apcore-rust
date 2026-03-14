// APCore Protocol — Middleware base trait
// Spec reference: Middleware lifecycle (before, after, on_error)

use async_trait::async_trait;

use crate::context::Context;
use crate::errors::ModuleError;

/// Core middleware trait with before/after/on_error hooks.
///
/// All hooks return `Option<Value>`:
/// - `Some(value)` means the middleware modified the input/output/recovery value.
/// - `None` means "no modification" — the pipeline keeps the previous value.
///
/// `on_error` returns `Option<Value>` where `Some(value)` signals a recovery
/// (the pipeline should retry with the returned inputs) and `None` means
/// the error should propagate.
#[async_trait]
pub trait Middleware: Send + Sync + std::fmt::Debug {
    /// Name of this middleware for logging/debugging.
    fn name(&self) -> &str;

    /// Called before module execution. Can modify input.
    /// Return `Ok(None)` to pass through unchanged, `Ok(Some(v))` to modify.
    async fn before(
        &self,
        module_id: &str,
        inputs: serde_json::Value,
        ctx: &Context<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, ModuleError>;

    /// Called after successful module execution. Can modify output.
    /// `inputs` is the original (post-before) input for correlation.
    /// Return `Ok(None)` to pass through unchanged, `Ok(Some(v))` to modify.
    async fn after(
        &self,
        module_id: &str,
        inputs: serde_json::Value,
        output: serde_json::Value,
        ctx: &Context<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, ModuleError>;

    /// Called when module execution fails.
    /// `inputs` is the original (post-before) input for correlation.
    /// Return `Ok(Some(v))` to signal recovery (retry with those inputs),
    /// or `Ok(None)` to let the error propagate.
    async fn on_error(
        &self,
        module_id: &str,
        inputs: serde_json::Value,
        error: &ModuleError,
        ctx: &Context<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, ModuleError>;
}

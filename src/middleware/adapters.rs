// APCore Protocol — Middleware adapter traits
// Spec reference: Simplified before-only and after-only middleware

use async_trait::async_trait;

use crate::context::Context;
use crate::errors::ModuleError;

/// Adapter for middleware that only needs a before hook.
#[async_trait]
pub trait BeforeMiddleware: Send + Sync + std::fmt::Debug {
    fn name(&self) -> &str;

    async fn before(
        &self,
        module_id: &str,
        inputs: serde_json::Value,
        ctx: &Context<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, ModuleError>;
}

/// Adapter for middleware that only needs an after hook.
#[async_trait]
pub trait AfterMiddleware: Send + Sync + std::fmt::Debug {
    fn name(&self) -> &str;

    async fn after(
        &self,
        module_id: &str,
        inputs: serde_json::Value,
        output: serde_json::Value,
        ctx: &Context<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, ModuleError>;
}

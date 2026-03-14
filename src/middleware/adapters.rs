// APCore Protocol — Middleware adapters
// Spec reference: Simplified before-only and after-only middleware

use async_trait::async_trait;

use super::base::Middleware;
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

/// Wraps a BeforeMiddleware into a full Middleware.
#[derive(Debug)]
pub struct BeforeMiddlewareAdapter<T: BeforeMiddleware>(pub T);

#[async_trait]
impl<T: BeforeMiddleware + 'static> Middleware for BeforeMiddlewareAdapter<T> {
    fn name(&self) -> &str {
        self.0.name()
    }

    async fn before(
        &self,
        module_id: &str,
        inputs: serde_json::Value,
        ctx: &Context<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, ModuleError> {
        self.0.before(module_id, inputs, ctx).await
    }

    async fn after(
        &self,
        _module_id: &str,
        _inputs: serde_json::Value,
        _output: serde_json::Value,
        _ctx: &Context<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, ModuleError> {
        Ok(None)
    }

    async fn on_error(
        &self,
        _module_id: &str,
        _inputs: serde_json::Value,
        _error: &ModuleError,
        _ctx: &Context<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, ModuleError> {
        Ok(None)
    }
}

/// Wraps an AfterMiddleware into a full Middleware.
#[derive(Debug)]
pub struct AfterMiddlewareAdapter<T: AfterMiddleware>(pub T);

#[async_trait]
impl<T: AfterMiddleware + 'static> Middleware for AfterMiddlewareAdapter<T> {
    fn name(&self) -> &str {
        self.0.name()
    }

    async fn before(
        &self,
        _module_id: &str,
        _inputs: serde_json::Value,
        _ctx: &Context<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, ModuleError> {
        Ok(None)
    }

    async fn after(
        &self,
        module_id: &str,
        inputs: serde_json::Value,
        output: serde_json::Value,
        ctx: &Context<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, ModuleError> {
        self.0.after(module_id, inputs, output, ctx).await
    }

    async fn on_error(
        &self,
        _module_id: &str,
        _inputs: serde_json::Value,
        _error: &ModuleError,
        _ctx: &Context<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, ModuleError> {
        Ok(None)
    }
}

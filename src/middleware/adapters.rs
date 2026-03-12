// APCore Protocol — Middleware adapters
// Spec reference: Simplified before-only and after-only middleware

use async_trait::async_trait;

use crate::context::Context;
use crate::errors::ModuleError;
use super::base::Middleware;

/// Adapter for middleware that only needs a before hook.
#[async_trait]
pub trait BeforeMiddleware: Send + Sync + std::fmt::Debug {
    fn name(&self) -> &str;

    async fn before(
        &self,
        ctx: &Context<serde_json::Value>,
        module_name: &str,
        input: serde_json::Value,
    ) -> Result<serde_json::Value, ModuleError>;
}

/// Adapter for middleware that only needs an after hook.
#[async_trait]
pub trait AfterMiddleware: Send + Sync + std::fmt::Debug {
    fn name(&self) -> &str;

    async fn after(
        &self,
        ctx: &Context<serde_json::Value>,
        module_name: &str,
        inputs: serde_json::Value,
        output: serde_json::Value,
    ) -> Result<serde_json::Value, ModuleError>;
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
        ctx: &Context<serde_json::Value>,
        module_name: &str,
        input: serde_json::Value,
    ) -> Result<serde_json::Value, ModuleError> {
        self.0.before(ctx, module_name, input).await
    }

    async fn after(
        &self,
        _ctx: &Context<serde_json::Value>,
        _module_name: &str,
        _inputs: serde_json::Value,
        output: serde_json::Value,
    ) -> Result<serde_json::Value, ModuleError> {
        Ok(output)
    }

    async fn on_error(
        &self,
        _ctx: &Context<serde_json::Value>,
        _module_name: &str,
        _inputs: serde_json::Value,
        _error: &ModuleError,
    ) -> Result<(), ModuleError> {
        Ok(())
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
        _ctx: &Context<serde_json::Value>,
        _module_name: &str,
        input: serde_json::Value,
    ) -> Result<serde_json::Value, ModuleError> {
        Ok(input)
    }

    async fn after(
        &self,
        ctx: &Context<serde_json::Value>,
        module_name: &str,
        inputs: serde_json::Value,
        output: serde_json::Value,
    ) -> Result<serde_json::Value, ModuleError> {
        self.0.after(ctx, module_name, inputs, output).await
    }

    async fn on_error(
        &self,
        _ctx: &Context<serde_json::Value>,
        _module_name: &str,
        _inputs: serde_json::Value,
        _error: &ModuleError,
    ) -> Result<(), ModuleError> {
        Ok(())
    }
}

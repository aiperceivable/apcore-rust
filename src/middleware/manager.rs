// APCore Protocol — Middleware manager
// Spec reference: Middleware pipeline execution

use super::base::Middleware;
use crate::context::Context;
use crate::errors::ModuleError;

/// Manages an ordered pipeline of middleware.
#[derive(Debug)]
pub struct MiddlewareManager {
    middlewares: Vec<Box<dyn Middleware>>,
}

impl MiddlewareManager {
    /// Create a new empty middleware manager.
    pub fn new() -> Self {
        Self {
            middlewares: vec![],
        }
    }

    /// Add a middleware to the pipeline.
    pub fn add(&mut self, middleware: Box<dyn Middleware>) {
        self.middlewares.push(middleware);
    }

    /// Remove a middleware by name.
    pub fn remove(&mut self, name: &str) -> bool {
        let len_before = self.middlewares.len();
        self.middlewares.retain(|m| m.name() != name);
        self.middlewares.len() < len_before
    }

    /// Run the before hooks for all middlewares in order.
    pub async fn run_before(
        &self,
        ctx: &Context<serde_json::Value>,
        module_name: &str,
        mut input: serde_json::Value,
    ) -> Result<serde_json::Value, ModuleError> {
        // TODO: Implement
        todo!()
    }

    /// Run the after hooks for all middlewares in reverse order.
    pub async fn run_after(
        &self,
        ctx: &Context<serde_json::Value>,
        module_name: &str,
        inputs: serde_json::Value,
        mut output: serde_json::Value,
    ) -> Result<serde_json::Value, ModuleError> {
        // TODO: Implement
        todo!()
    }

    /// Run the on_error hooks for all middlewares.
    pub async fn run_on_error(
        &self,
        ctx: &Context<serde_json::Value>,
        module_name: &str,
        inputs: serde_json::Value,
        error: &ModuleError,
    ) -> Result<(), ModuleError> {
        // TODO: Implement
        todo!()
    }

    /// List middleware names in pipeline order.
    pub fn list(&self) -> Vec<&str> {
        self.middlewares.iter().map(|m| m.name()).collect()
    }
}

impl Default for MiddlewareManager {
    fn default() -> Self {
        Self::new()
    }
}

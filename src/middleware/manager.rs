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

    /// Remove the first middleware matching the given name.
    ///
    /// Removes only the first match (not all), matching Python's
    /// identity-based semantics which also removes exactly one instance.
    pub fn remove(&mut self, name: &str) -> bool {
        let pos = self.middlewares.iter().position(|m| m.name() == name);
        if let Some(i) = pos {
            self.middlewares.remove(i);
            true
        } else {
            false
        }
    }

    /// Run the before hooks for all middlewares in order.
    ///
    /// Returns the (possibly modified) input and the list of indices of
    /// middlewares that were successfully executed (used by `execute_on_error`
    /// for onion-model unwinding).
    pub async fn execute_before(
        &self,
        ctx: &Context<serde_json::Value>,
        module_name: &str,
        mut input: serde_json::Value,
    ) -> Result<(serde_json::Value, Vec<usize>), ModuleError> {
        let mut executed: Vec<usize> = Vec::new();
        for (i, mw) in self.middlewares.iter().enumerate() {
            input = mw.before(ctx, module_name, input).await?;
            executed.push(i);
        }
        Ok((input, executed))
    }

    /// Run the after hooks for all middlewares in reverse order (onion model).
    pub async fn execute_after(
        &self,
        ctx: &Context<serde_json::Value>,
        module_name: &str,
        inputs: serde_json::Value,
        mut output: serde_json::Value,
    ) -> Result<serde_json::Value, ModuleError> {
        for mw in self.middlewares.iter().rev() {
            output = mw.after(ctx, module_name, inputs.clone(), output).await?;
        }
        Ok(output)
    }

    /// Run the on_error hooks in reverse order over the middlewares that
    /// were executed during `execute_before`.
    ///
    /// The first middleware whose `on_error` succeeds without returning an
    /// error is considered a recovery — but we still call all remaining
    /// middlewares for cleanup, matching the Python onion-model unwinding.
    pub async fn execute_on_error(
        &self,
        ctx: &Context<serde_json::Value>,
        module_name: &str,
        inputs: serde_json::Value,
        error: &ModuleError,
        executed: &[usize],
    ) -> Result<(), ModuleError> {
        for &i in executed.iter().rev() {
            if let Some(mw) = self.middlewares.get(i) {
                // Best-effort: log but don't propagate individual on_error failures
                if let Err(e) = mw.on_error(ctx, module_name, inputs.clone(), error).await {
                    eprintln!("Middleware '{}' on_error failed: {}", mw.name(), e);
                }
            }
        }
        Ok(())
    }

    /// Snapshot of middleware names in pipeline order.
    pub fn snapshot(&self) -> Vec<&str> {
        self.middlewares.iter().map(|m| m.name()).collect()
    }
}

impl Default for MiddlewareManager {
    fn default() -> Self {
        Self::new()
    }
}

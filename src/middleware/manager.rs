// APCore Protocol — Middleware manager
// Spec reference: Middleware pipeline execution

use std::sync::{Arc, Mutex};

use super::base::Middleware;
use crate::context::Context;
use crate::errors::{ErrorCode, ModuleError};

/// Manages an ordered pipeline of middleware.
///
/// Thread-safe: internal list is protected by a Mutex. All execution methods
/// clone the Arc pointers out of the mutex before iterating so the lock is
/// never held across `.await` points.
#[derive(Debug)]
pub struct MiddlewareManager {
    middlewares: Mutex<Vec<Arc<dyn Middleware>>>,
}

impl MiddlewareManager {
    /// Create a new empty middleware manager.
    pub fn new() -> Self {
        Self {
            middlewares: Mutex::new(vec![]),
        }
    }

    /// Add a middleware to the pipeline.
    pub fn add(&mut self, middleware: Box<dyn Middleware>) {
        let mut mws = self.middlewares.lock().unwrap_or_else(|e| e.into_inner());
        mws.push(Arc::from(middleware));
    }

    /// Remove the first middleware matching the given name.
    ///
    /// Removes only the first match (not all), matching Python's
    /// identity-based semantics which also removes exactly one instance.
    pub fn remove(&mut self, name: &str) -> bool {
        let mut mws = self.middlewares.lock().unwrap_or_else(|e| e.into_inner());
        let pos = mws.iter().position(|m| m.name() == name);
        if let Some(i) = pos {
            mws.remove(i);
            true
        } else {
            false
        }
    }

    /// Return a snapshot of middleware names in pipeline order.
    pub fn snapshot(&self) -> Vec<String> {
        let mws = self.middlewares.lock().unwrap_or_else(|e| e.into_inner());
        mws.iter().map(|m| m.name().to_string()).collect()
    }

    /// Run the before hooks for all middlewares in order.
    ///
    /// Returns the (possibly modified) input and the list of indices of
    /// middlewares that were successfully executed (used by `execute_on_error`
    /// for onion-model unwinding).
    ///
    /// If a middleware's before() fails, wraps the error as a
    /// MiddlewareChainError and returns it. The `executed` list allows
    /// callers to run on_error only on middlewares that actually ran.
    pub async fn execute_before(
        &self,
        module_id: &str,
        mut inputs: serde_json::Value,
        ctx: &Context<serde_json::Value>,
    ) -> Result<(serde_json::Value, Vec<usize>), ModuleError> {
        // Clone Arc pointers so the mutex is not held across .await points.
        let mws: Vec<Arc<dyn Middleware>> = {
            let guard = self.middlewares.lock().unwrap_or_else(|e| e.into_inner());
            guard.iter().map(Arc::clone).collect()
        };
        let mut executed: Vec<usize> = Vec::new();
        for (i, mw) in mws.iter().enumerate() {
            match mw.before(module_id, inputs.clone(), ctx).await {
                Ok(Some(modified)) => {
                    inputs = modified;
                    executed.push(i);
                }
                Ok(None) => {
                    // No modification — keep current inputs
                    executed.push(i);
                }
                Err(e) => {
                    return Err(ModuleError::new(
                        ErrorCode::MiddlewareChainError,
                        e.message.clone(),
                    )
                    .with_cause(format!(
                        "Middleware '{}' before() failed: {}",
                        mw.name(),
                        e
                    )));
                }
            }
        }
        Ok((inputs, executed))
    }

    /// Run the after hooks for all middlewares in reverse order (onion model).
    ///
    /// If a middleware returns `Some(value)`, the output is replaced.
    /// If it returns `None`, the current output is kept unchanged.
    pub async fn execute_after(
        &self,
        module_id: &str,
        inputs: serde_json::Value,
        mut output: serde_json::Value,
        ctx: &Context<serde_json::Value>,
    ) -> Result<serde_json::Value, ModuleError> {
        // Clone Arc pointers so the mutex is not held across .await points.
        let mws: Vec<Arc<dyn Middleware>> = {
            let guard = self.middlewares.lock().unwrap_or_else(|e| e.into_inner());
            guard.iter().map(Arc::clone).collect()
        };
        for mw in mws.iter().rev() {
            match mw
                .after(module_id, inputs.clone(), output.clone(), ctx)
                .await?
            {
                Some(modified) => {
                    output = modified;
                }
                None => {
                    // No modification — keep current output
                }
            }
        }
        Ok(output)
    }

    /// Run the on_error hooks in reverse order over the middlewares that
    /// were executed during `execute_before`.
    ///
    /// Returns `Ok(Some(recovery))` if any middleware provides a recovery
    /// value, or `Ok(None)` if no handler recovers. All executed middlewares
    /// are called for cleanup even after a recovery is found, but only the
    /// first recovery value is returned — matching the Python onion-model.
    pub async fn execute_on_error(
        &self,
        module_id: &str,
        inputs: serde_json::Value,
        error: &ModuleError,
        ctx: &Context<serde_json::Value>,
        executed: &[usize],
    ) -> Option<serde_json::Value> {
        // Clone Arc pointers so the mutex is not held across .await points.
        let mws: Vec<Arc<dyn Middleware>> = {
            let guard = self.middlewares.lock().unwrap_or_else(|e| e.into_inner());
            guard.iter().map(Arc::clone).collect()
        };
        let mut recovery: Option<serde_json::Value> = None;

        for &i in executed.iter().rev() {
            if let Some(mw) = mws.get(i) {
                match mw.on_error(module_id, inputs.clone(), error, ctx).await {
                    Ok(Some(value)) => {
                        if recovery.is_none() {
                            recovery = Some(value);
                        }
                    }
                    Ok(None) => {
                        // No recovery from this middleware
                    }
                    Err(e) => {
                        tracing::error!("Middleware '{}' on_error failed: {}", mw.name(), e);
                    }
                }
            }
        }

        recovery
    }
}

impl Default for MiddlewareManager {
    fn default() -> Self {
        Self::new()
    }
}

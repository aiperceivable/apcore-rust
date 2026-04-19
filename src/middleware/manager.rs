// APCore Protocol — Middleware manager
// Spec reference: Middleware pipeline execution

use std::sync::Arc;

use parking_lot::Mutex;

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
    #[must_use]
    pub fn new() -> Self {
        Self {
            middlewares: Mutex::new(vec![]),
        }
    }

    /// Add a middleware to the pipeline.
    ///
    /// Middlewares are maintained in sorted order by priority (higher first).
    /// Among middlewares with the same priority, registration order is preserved.
    ///
    /// Returns an error if the middleware's priority exceeds 1000 (the spec-defined
    /// maximum from section 11.2).
    ///
    /// Takes `&self` — the internal list is protected by a `Mutex`, so mutation
    /// is possible through a shared reference. This allows `MiddlewareManager`
    /// to be held as `Arc<MiddlewareManager>` and mutated without `Arc::get_mut`
    /// hacks, even after the `Arc` has been cloned into pipeline contexts.
    pub fn add(&self, middleware: Box<dyn Middleware>) -> Result<(), ModuleError> {
        let priority = middleware.priority();
        if priority > 1000 {
            tracing::warn!(
                middleware = middleware.name(),
                priority = priority,
                "Middleware rejected: priority {} exceeds maximum 1000",
                priority,
            );
            return Err(ModuleError::new(
                ErrorCode::GeneralInvalidInput,
                format!(
                    "Middleware '{}' has priority {} which exceeds the maximum allowed value of 1000",
                    middleware.name(),
                    priority,
                ),
            ));
        }
        let mut mws = self.middlewares.lock();
        let arc: Arc<dyn Middleware> = Arc::from(middleware);
        // Find the first position where existing priority is strictly less than
        // the new priority. Insert before that position to maintain stable
        // ordering (later registrations go after earlier ones at same priority).
        let pos = mws
            .iter()
            .position(|m| m.priority() < priority)
            .unwrap_or(mws.len());
        mws.insert(pos, arc);
        Ok(())
    }

    /// Remove the first middleware matching the given name.
    ///
    /// Removes only the first match (not all), matching Python's
    /// identity-based semantics which also removes exactly one instance.
    ///
    /// Takes `&self` — mutation goes through the internal `Mutex`.
    pub fn remove(&self, name: &str) -> bool {
        let mut mws = self.middlewares.lock();
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
        let mws = self.middlewares.lock();
        mws.iter().map(|m| m.name().to_string()).collect()
    }

    /// Run the before hooks for all middlewares in order.
    ///
    /// Returns the (possibly modified) input and the list of indices of
    /// middlewares that were successfully executed (used by `execute_on_error`
    /// for onion-model unwinding).
    ///
    /// If a middleware's `before()` fails, wraps the error as a
    /// `MiddlewareChainError` and returns it. The `executed` list allows
    /// callers to run `on_error` only on middlewares that actually ran.
    pub async fn execute_before(
        &self,
        module_id: &str,
        mut inputs: serde_json::Value,
        ctx: &Context<serde_json::Value>,
    ) -> Result<(serde_json::Value, Vec<usize>), ModuleError> {
        // Clone Arc pointers so the mutex is not held across .await points.
        let mws: Vec<Arc<dyn Middleware>> = {
            let guard = self.middlewares.lock();
            guard.iter().map(Arc::clone).collect()
        };
        let mut executed: Vec<usize> = Vec::new();
        for (i, mw) in mws.iter().enumerate() {
            // Push BEFORE calling before() so the failing middleware is included in
            // executed — enabling on_error self-heal (mirrors Python/TS behaviour).
            executed.push(i);
            match mw.before(module_id, inputs.clone(), ctx).await {
                Ok(Some(modified)) => {
                    inputs = modified;
                }
                Ok(None) => {
                    // No modification — keep current inputs
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
            let guard = self.middlewares.lock();
            guard.iter().map(Arc::clone).collect()
        };
        for mw in mws.iter().rev() {
            if let Some(modified) = mw
                .after(module_id, inputs.clone(), output.clone(), ctx)
                .await?
            {
                output = modified;
            } else {
                // No modification — keep current output
            }
        }
        Ok(output)
    }

    /// Run the `on_error` hooks in reverse order over the middlewares that
    /// were executed during `execute_before`.
    ///
    /// Returns `Some(recovery)` if any middleware provides a recovery value.
    /// Returns `None` if no handler recovers.
    ///
    /// Early-returns on the first recovery value — subsequent middlewares'
    /// `on_error` hooks are NOT called after recovery (mirrors Python/TS).
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
            let guard = self.middlewares.lock();
            guard.iter().map(Arc::clone).collect()
        };

        for &i in executed.iter().rev() {
            if let Some(mw) = mws.get(i) {
                match mw.on_error(module_id, inputs.clone(), error, ctx).await {
                    Ok(Some(value)) => {
                        // Early-return on first recovery (matches Python/TS semantics).
                        return Some(value);
                    }
                    Ok(None) => {
                        // No recovery from this middleware — continue
                    }
                    Err(e) => {
                        tracing::error!("Middleware '{}' on_error failed: {}", mw.name(), e);
                    }
                }
            }
        }

        None
    }
}

impl Default for MiddlewareManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::Context;
    use crate::errors::ModuleError;
    use async_trait::async_trait;

    /// Simple test middleware with configurable name and priority.
    #[derive(Debug)]
    struct TestMiddleware {
        mw_name: String,
        mw_priority: u16,
    }

    impl TestMiddleware {
        fn new(name: &str, priority: u16) -> Self {
            Self {
                mw_name: name.to_string(),
                mw_priority: priority,
            }
        }

        fn boxed(name: &str, priority: u16) -> Box<Self> {
            Box::new(Self::new(name, priority))
        }
    }

    #[async_trait]
    impl Middleware for TestMiddleware {
        fn name(&self) -> &str {
            &self.mw_name
        }

        fn priority(&self) -> u16 {
            self.mw_priority
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

    #[test]
    fn test_higher_priority_executes_first_in_before() {
        let mgr = MiddlewareManager::new();
        mgr.add(TestMiddleware::boxed("low", 1)).unwrap();
        mgr.add(TestMiddleware::boxed("high", 10)).unwrap();
        mgr.add(TestMiddleware::boxed("mid", 5)).unwrap();

        let names = mgr.snapshot();
        assert_eq!(names, vec!["high", "mid", "low"]);
    }

    #[test]
    fn test_equal_priority_preserves_registration_order() {
        let mgr = MiddlewareManager::new();
        mgr.add(TestMiddleware::boxed("first", 5)).unwrap();
        mgr.add(TestMiddleware::boxed("second", 5)).unwrap();
        mgr.add(TestMiddleware::boxed("third", 5)).unwrap();

        let names = mgr.snapshot();
        assert_eq!(names, vec!["first", "second", "third"]);
    }

    #[test]
    fn test_default_priority_orders_after_explicit_priority() {
        let mgr = MiddlewareManager::new();
        mgr.add(TestMiddleware::boxed("default_a", 0)).unwrap();
        mgr.add(TestMiddleware::boxed("explicit", 1)).unwrap();
        mgr.add(TestMiddleware::boxed("default_b", 0)).unwrap();

        let names = mgr.snapshot();
        assert_eq!(names, vec!["explicit", "default_a", "default_b"]);
    }

    #[test]
    fn test_snapshot_reflects_priority_sorted_order() {
        let mgr = MiddlewareManager::new();
        mgr.add(TestMiddleware::boxed("d", 0)).unwrap();
        mgr.add(TestMiddleware::boxed("a", 100)).unwrap();
        mgr.add(TestMiddleware::boxed("c", 5)).unwrap();
        mgr.add(TestMiddleware::boxed("b", 50)).unwrap();

        let names = mgr.snapshot();
        assert_eq!(names, vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn test_add_rejects_priority_above_1000() {
        let mgr = MiddlewareManager::new();
        let result = mgr.add(TestMiddleware::boxed("over_limit", 1001));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.message.contains("exceeds the maximum"),
            "Expected error about priority limit, got: {}",
            err.message,
        );
        // Pipeline should be empty — the middleware was not added.
        assert!(mgr.snapshot().is_empty());
    }

    #[test]
    fn test_add_accepts_priority_at_1000() {
        let mgr = MiddlewareManager::new();
        mgr.add(TestMiddleware::boxed("at_limit", 1000)).unwrap();
        assert_eq!(mgr.snapshot(), vec!["at_limit"]);
    }
}

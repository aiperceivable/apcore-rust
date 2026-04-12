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

    /// Priority of this middleware (higher runs first). Default is 100.
    /// Valid range: 0-1000 (enforced by MiddlewareManager::add).
    /// When two middlewares have the same priority, registration order is preserved.
    fn priority(&self) -> u16 {
        100
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::ErrorCode;
    use serde_json::json;

    #[derive(Debug)]
    struct TestMiddleware {
        name: String,
        prio: u16,
    }

    impl TestMiddleware {
        fn new(name: &str, prio: u16) -> Self {
            Self {
                name: name.to_string(),
                prio,
            }
        }
    }

    #[async_trait]
    impl Middleware for TestMiddleware {
        fn name(&self) -> &str {
            &self.name
        }

        fn priority(&self) -> u16 {
            self.prio
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
    fn test_middleware_default_priority() {
        #[derive(Debug)]
        struct DefaultPrio;

        #[async_trait]
        impl Middleware for DefaultPrio {
            fn name(&self) -> &'static str {
                "default"
            }
            async fn before(
                &self,
                _: &str,
                _: serde_json::Value,
                _: &Context<serde_json::Value>,
            ) -> Result<Option<serde_json::Value>, ModuleError> {
                Ok(None)
            }
            async fn after(
                &self,
                _: &str,
                _: serde_json::Value,
                _: serde_json::Value,
                _: &Context<serde_json::Value>,
            ) -> Result<Option<serde_json::Value>, ModuleError> {
                Ok(None)
            }
            async fn on_error(
                &self,
                _: &str,
                _: serde_json::Value,
                _: &ModuleError,
                _: &Context<serde_json::Value>,
            ) -> Result<Option<serde_json::Value>, ModuleError> {
                Ok(None)
            }
        }

        let mw = DefaultPrio;
        assert_eq!(mw.priority(), 100);
    }

    #[test]
    fn test_middleware_custom_priority() {
        let mw = TestMiddleware::new("high_priority", 500);
        assert_eq!(mw.priority(), 500);
        assert_eq!(mw.name(), "high_priority");
    }

    #[tokio::test]
    async fn test_middleware_before_returns_none() {
        let mw = TestMiddleware::new("test", 100);
        let ctx = Context::<serde_json::Value>::anonymous();
        let result = mw.before("mod.a", json!({"x": 1}), &ctx).await.unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_middleware_after_returns_none() {
        let mw = TestMiddleware::new("test", 100);
        let ctx = Context::<serde_json::Value>::anonymous();
        let result = mw
            .after("mod.a", json!({}), json!({"result": true}), &ctx)
            .await
            .unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_middleware_on_error_returns_none() {
        let mw = TestMiddleware::new("test", 100);
        let ctx = Context::<serde_json::Value>::anonymous();
        let err = ModuleError::new(ErrorCode::ModuleExecuteError, "boom");
        let result = mw.on_error("mod.a", json!({}), &err, &ctx).await.unwrap();
        assert_eq!(result, None);
    }
}

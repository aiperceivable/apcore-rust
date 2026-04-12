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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Debug)]
    struct LoggingBeforeMiddleware;

    #[async_trait]
    impl BeforeMiddleware for LoggingBeforeMiddleware {
        fn name(&self) -> &'static str {
            "logging_before"
        }

        async fn before(
            &self,
            _module_id: &str,
            inputs: serde_json::Value,
            _ctx: &Context<serde_json::Value>,
        ) -> Result<Option<serde_json::Value>, ModuleError> {
            Ok(Some(inputs))
        }
    }

    #[derive(Debug)]
    struct LoggingAfterMiddleware;

    #[async_trait]
    impl AfterMiddleware for LoggingAfterMiddleware {
        fn name(&self) -> &'static str {
            "logging_after"
        }

        async fn after(
            &self,
            _module_id: &str,
            _inputs: serde_json::Value,
            output: serde_json::Value,
            _ctx: &Context<serde_json::Value>,
        ) -> Result<Option<serde_json::Value>, ModuleError> {
            Ok(Some(output))
        }
    }

    #[tokio::test]
    async fn test_before_middleware_name() {
        let mw = LoggingBeforeMiddleware;
        assert_eq!(mw.name(), "logging_before");
    }

    #[tokio::test]
    async fn test_before_middleware_passthrough() {
        let mw = LoggingBeforeMiddleware;
        let ctx = Context::<serde_json::Value>::anonymous();
        let inputs = json!({"key": "value"});
        let result = mw
            .before("test.module", inputs.clone(), &ctx)
            .await
            .unwrap();
        assert_eq!(result, Some(inputs));
    }

    #[tokio::test]
    async fn test_after_middleware_name() {
        let mw = LoggingAfterMiddleware;
        assert_eq!(mw.name(), "logging_after");
    }

    #[tokio::test]
    async fn test_after_middleware_passthrough() {
        let mw = LoggingAfterMiddleware;
        let ctx = Context::<serde_json::Value>::anonymous();
        let inputs = json!({"in": 1});
        let output = json!({"out": 2});
        let result = mw
            .after("test.module", inputs, output.clone(), &ctx)
            .await
            .unwrap();
        assert_eq!(result, Some(output));
    }

    #[tokio::test]
    async fn test_before_middleware_returns_none() {
        #[derive(Debug)]
        struct NoOpBefore;

        #[async_trait]
        impl BeforeMiddleware for NoOpBefore {
            fn name(&self) -> &'static str {
                "noop"
            }
            async fn before(
                &self,
                _module_id: &str,
                _inputs: serde_json::Value,
                _ctx: &Context<serde_json::Value>,
            ) -> Result<Option<serde_json::Value>, ModuleError> {
                Ok(None)
            }
        }

        let mw = NoOpBefore;
        let ctx = Context::<serde_json::Value>::anonymous();
        let result = mw.before("m", json!({}), &ctx).await.unwrap();
        assert_eq!(result, None);
    }
}

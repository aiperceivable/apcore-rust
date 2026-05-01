// APCore Protocol — Declarative middleware configuration (Issue #42)
// Spec reference: middleware-system.md §1.4 Declarative Middleware
// Configuration (YAML-Driven)
//
// Parses the `middleware:` array from `apcore.yaml` (or any equivalent YAML/
// JSON document) into a typed list of middleware factories. The built-in
// types are `tracing`, `circuit_breaker`, and `logging`; the `custom` type
// allows pointing at a user-registered factory by name.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::base::Middleware;
use super::circuit_breaker::{CircuitBreakerBuilder, CircuitBreakerMiddleware};
use super::logging::LoggingMiddleware;
use super::otel_tracing::{TracingBuilder, TracingMiddleware};
use crate::context::Context;
use crate::errors::{ErrorCode, ModuleError};
use crate::events::emitter::EventEmitter;

/// Configuration for the built-in `tracing` middleware type.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TracingMiddlewareConfig {
    pub service_name: Option<String>,
    pub propagate_traceparent: Option<bool>,
    pub priority: Option<u16>,
    /// Runtime override for the compile-time `opentelemetry` feature.
    pub enabled: Option<bool>,
    /// Optional glob patterns scoping the middleware to a subset of modules.
    /// Currently informational; future work may apply filtering at the
    /// pipeline level.
    pub match_modules: Option<Vec<String>>,
}

/// Configuration for the built-in `circuit_breaker` middleware type.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CircuitBreakerMiddlewareConfig {
    pub open_threshold: Option<f64>,
    pub window_size: Option<usize>,
    pub recovery_window_ms: Option<u64>,
    pub min_samples: Option<usize>,
    pub priority: Option<u16>,
    pub match_modules: Option<Vec<String>>,
}

/// Configuration for the built-in `logging` middleware type.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct LoggingMiddlewareConfig {
    pub log_inputs: Option<bool>,
    pub log_outputs: Option<bool>,
    pub log_errors: Option<bool>,
    pub priority: Option<u16>,
    pub match_modules: Option<Vec<String>>,
}

/// Configuration for a `custom` middleware referenced by registered name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomMiddlewareConfig {
    /// Registered factory name (e.g. `"myapp.middleware.RateLimiter"`).
    pub handler: String,
    #[serde(default)]
    pub config: serde_json::Value,
    #[serde(default)]
    pub match_modules: Option<Vec<String>>,
    #[serde(default)]
    pub priority: Option<u16>,
}

/// Tagged enum for the `type` field on each middleware entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MiddlewareConfig {
    Tracing(TracingMiddlewareConfig),
    CircuitBreaker(CircuitBreakerMiddlewareConfig),
    Logging(LoggingMiddlewareConfig),
    Custom(CustomMiddlewareConfig),
}

/// Wrapper for the `middleware:` top-level YAML array.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MiddlewareChainConfig {
    #[serde(default)]
    pub middleware: Vec<MiddlewareConfig>,
}

impl MiddlewareChainConfig {
    /// Parse a YAML document into a chain config.
    pub fn from_yaml(source: &str) -> Result<Self, ModuleError> {
        serde_yaml_ng::from_str(source).map_err(|e| {
            ModuleError::new(
                ErrorCode::PipelineConfigInvalid,
                format!("Invalid middleware YAML: {e}"),
            )
        })
    }

    /// Parse a JSON document into a chain config (useful for inline tests
    /// and conformance fixtures that ship JSON rather than YAML).
    pub fn from_json(source: &str) -> Result<Self, ModuleError> {
        serde_json::from_str(source).map_err(ModuleError::from)
    }
}

/// Factory function for a custom middleware type registered at runtime.
pub type CustomMiddlewareFactory =
    Arc<dyn Fn(&serde_json::Value) -> Result<Box<dyn Middleware>, ModuleError> + Send + Sync>;

/// Resolves [`MiddlewareConfig`] entries to concrete middleware instances.
///
/// `event_emitter` is wired into middlewares that emit lifecycle events
/// (currently `circuit_breaker`). When `None`, those middlewares run without
/// emitting events.
#[derive(Default)]
pub struct MiddlewareFactory {
    custom_factories: std::collections::HashMap<String, CustomMiddlewareFactory>,
    event_emitter: Option<Arc<EventEmitter>>,
}

impl std::fmt::Debug for MiddlewareFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MiddlewareFactory")
            .field("custom_factory_count", &self.custom_factories.len())
            .field("has_event_emitter", &self.event_emitter.is_some())
            .finish()
    }
}

impl MiddlewareFactory {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_event_emitter(mut self, emitter: Arc<EventEmitter>) -> Self {
        self.event_emitter = Some(emitter);
        self
    }

    /// Register a custom middleware factory by `handler` name.
    pub fn register_custom(
        &mut self,
        handler: impl Into<String>,
        factory: CustomMiddlewareFactory,
    ) {
        self.custom_factories.insert(handler.into(), factory);
    }

    /// Build a single middleware instance from a config entry.
    pub fn build(&self, config: &MiddlewareConfig) -> Result<Box<dyn Middleware>, ModuleError> {
        match config {
            MiddlewareConfig::Tracing(cfg) => Ok(Box::new(Self::build_tracing(cfg))),
            MiddlewareConfig::CircuitBreaker(cfg) => Ok(Box::new(self.build_circuit_breaker(cfg))),
            MiddlewareConfig::Logging(cfg) => Ok(Box::new(Self::build_logging(cfg))),
            MiddlewareConfig::Custom(cfg) => self.build_custom(cfg),
        }
    }

    /// Build the entire chain in declaration order.
    pub fn build_chain(
        &self,
        chain: &MiddlewareChainConfig,
    ) -> Result<Vec<Box<dyn Middleware>>, ModuleError> {
        chain.middleware.iter().map(|c| self.build(c)).collect()
    }

    fn build_tracing(cfg: &TracingMiddlewareConfig) -> TracingMiddleware {
        let mut b = TracingBuilder::default();
        if let Some(ref name) = cfg.service_name {
            b = b.service_name(name.clone());
        }
        if let Some(p) = cfg.propagate_traceparent {
            b = b.propagate_traceparent(p);
        }
        if let Some(p) = cfg.priority {
            b = b.priority(p);
        }
        if let Some(en) = cfg.enabled {
            b = b.enabled(en);
        }
        b.build()
    }

    fn build_circuit_breaker(
        &self,
        cfg: &CircuitBreakerMiddlewareConfig,
    ) -> CircuitBreakerMiddleware {
        let mut b = CircuitBreakerBuilder::default();
        if let Some(t) = cfg.open_threshold {
            b = b.open_threshold(t);
        }
        if let Some(w) = cfg.window_size {
            b = b.window_size(w);
        }
        if let Some(r) = cfg.recovery_window_ms {
            b = b.recovery_window_ms(r);
        }
        if let Some(s) = cfg.min_samples {
            b = b.min_samples(s);
        }
        if let Some(p) = cfg.priority {
            b = b.priority(p);
        }
        if let Some(emitter) = &self.event_emitter {
            b = b.emitter(Arc::clone(emitter));
        }
        b.build()
    }

    fn build_logging(cfg: &LoggingMiddlewareConfig) -> LoggingMiddleware {
        LoggingMiddleware::new(
            cfg.log_inputs.unwrap_or(true),
            cfg.log_outputs.unwrap_or(true),
            cfg.log_errors.unwrap_or(true),
        )
    }

    fn build_custom(
        &self,
        cfg: &CustomMiddlewareConfig,
    ) -> Result<Box<dyn Middleware>, ModuleError> {
        let factory = self.custom_factories.get(&cfg.handler).ok_or_else(|| {
            ModuleError::new(
                ErrorCode::PipelineConfigInvalid,
                format!(
                    "Custom middleware handler '{}' is not registered. \
                     Register it via MiddlewareFactory::register_custom() first.",
                    cfg.handler
                ),
            )
        })?;
        let inner = factory(&cfg.config)?;
        // Honor the YAML `priority` override by wrapping the user middleware.
        Ok(match cfg.priority {
            Some(p) => Box::new(PriorityOverride::new(inner, p)),
            None => inner,
        })
    }
}

/// Transparent wrapper that overrides the inner middleware's `priority()`
/// while delegating every other method. Created by [`MiddlewareFactory`] when
/// a YAML entry sets `priority` on a `custom` handler.
struct PriorityOverride {
    inner: Box<dyn Middleware>,
    priority: u16,
}

impl PriorityOverride {
    fn new(inner: Box<dyn Middleware>, priority: u16) -> Self {
        Self { inner, priority }
    }
}

impl std::fmt::Debug for PriorityOverride {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PriorityOverride")
            .field("priority", &self.priority)
            .field("inner_name", &self.inner.name())
            .finish()
    }
}

#[async_trait]
impl Middleware for PriorityOverride {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn priority(&self) -> u16 {
        self.priority
    }

    async fn before(
        &self,
        module_id: &str,
        inputs: serde_json::Value,
        ctx: &Context<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, ModuleError> {
        self.inner.before(module_id, inputs, ctx).await
    }

    async fn after(
        &self,
        module_id: &str,
        inputs: serde_json::Value,
        output: serde_json::Value,
        ctx: &Context<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, ModuleError> {
        self.inner.after(module_id, inputs, output, ctx).await
    }

    async fn on_error(
        &self,
        module_id: &str,
        inputs: serde_json::Value,
        error: &ModuleError,
        ctx: &Context<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, ModuleError> {
        self.inner.on_error(module_id, inputs, error, ctx).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tracing_entry() {
        let yaml = r#"
middleware:
  - type: tracing
    service_name: "my-service"
    propagate_traceparent: true
    enabled: true
"#;
        let chain = MiddlewareChainConfig::from_yaml(yaml).unwrap();
        assert_eq!(chain.middleware.len(), 1);
        match &chain.middleware[0] {
            MiddlewareConfig::Tracing(cfg) => {
                assert_eq!(cfg.service_name.as_deref(), Some("my-service"));
                assert_eq!(cfg.propagate_traceparent, Some(true));
                assert_eq!(cfg.enabled, Some(true));
            }
            _ => panic!("expected tracing config"),
        }
    }

    #[test]
    fn parses_circuit_breaker_entry() {
        let yaml = r"
middleware:
  - type: circuit_breaker
    open_threshold: 0.3
    recovery_window_ms: 60000
    window_size: 20
";
        let chain = MiddlewareChainConfig::from_yaml(yaml).unwrap();
        match &chain.middleware[0] {
            MiddlewareConfig::CircuitBreaker(cfg) => {
                assert_eq!(cfg.open_threshold, Some(0.3));
                assert_eq!(cfg.recovery_window_ms, Some(60_000));
                assert_eq!(cfg.window_size, Some(20));
            }
            _ => panic!("expected circuit_breaker config"),
        }
    }

    #[test]
    fn parses_logging_entry() {
        let yaml = r"
middleware:
  - type: logging
    log_inputs: true
    log_outputs: false
";
        let chain = MiddlewareChainConfig::from_yaml(yaml).unwrap();
        match &chain.middleware[0] {
            MiddlewareConfig::Logging(cfg) => {
                assert_eq!(cfg.log_inputs, Some(true));
                assert_eq!(cfg.log_outputs, Some(false));
            }
            _ => panic!("expected logging config"),
        }
    }

    #[test]
    fn parses_custom_entry() {
        let yaml = r#"
middleware:
  - type: custom
    handler: "myapp.middleware.RateLimiter"
    config:
      requests_per_second: 100
"#;
        let chain = MiddlewareChainConfig::from_yaml(yaml).unwrap();
        match &chain.middleware[0] {
            MiddlewareConfig::Custom(cfg) => {
                assert_eq!(cfg.handler, "myapp.middleware.RateLimiter");
                assert_eq!(
                    cfg.config
                        .get("requests_per_second")
                        .and_then(serde_json::Value::as_u64),
                    Some(100)
                );
            }
            _ => panic!("expected custom config"),
        }
    }

    #[test]
    fn factory_builds_tracing_and_logging() {
        let yaml = r"
middleware:
  - type: tracing
    enabled: false
  - type: logging
";
        let chain = MiddlewareChainConfig::from_yaml(yaml).unwrap();
        let factory = MiddlewareFactory::new();
        let built = factory.build_chain(&chain).unwrap();
        assert_eq!(built.len(), 2);
        assert_eq!(built[0].name(), "tracing");
        assert_eq!(built[1].name(), "logging");
    }

    #[test]
    fn factory_unknown_custom_handler_errors() {
        let yaml = r#"
middleware:
  - type: custom
    handler: "missing.handler"
"#;
        let chain = MiddlewareChainConfig::from_yaml(yaml).unwrap();
        let factory = MiddlewareFactory::new();
        let err = factory.build_chain(&chain).unwrap_err();
        assert_eq!(err.code, ErrorCode::PipelineConfigInvalid);
    }

    #[test]
    fn factory_resolves_registered_custom_handler() {
        use async_trait::async_trait;

        #[derive(Debug)]
        struct StubMw;

        #[async_trait]
        impl Middleware for StubMw {
            fn name(&self) -> &'static str {
                "stub"
            }
            async fn before(
                &self,
                _: &str,
                _: serde_json::Value,
                _: &crate::context::Context<serde_json::Value>,
            ) -> Result<Option<serde_json::Value>, ModuleError> {
                Ok(None)
            }
            async fn after(
                &self,
                _: &str,
                _: serde_json::Value,
                _: serde_json::Value,
                _: &crate::context::Context<serde_json::Value>,
            ) -> Result<Option<serde_json::Value>, ModuleError> {
                Ok(None)
            }
            async fn on_error(
                &self,
                _: &str,
                _: serde_json::Value,
                _: &ModuleError,
                _: &crate::context::Context<serde_json::Value>,
            ) -> Result<Option<serde_json::Value>, ModuleError> {
                Ok(None)
            }
        }

        let yaml = r#"
middleware:
  - type: custom
    handler: "stub"
"#;
        let chain = MiddlewareChainConfig::from_yaml(yaml).unwrap();
        let mut factory = MiddlewareFactory::new();
        factory.register_custom(
            "stub",
            Arc::new(|_cfg| Ok(Box::new(StubMw) as Box<dyn Middleware>)),
        );
        let built = factory.build_chain(&chain).unwrap();
        assert_eq!(built[0].name(), "stub");
    }

    #[test]
    fn factory_applies_priority_override_to_custom_middleware() {
        use async_trait::async_trait;

        #[derive(Debug)]
        struct StubMw;

        #[async_trait]
        impl Middleware for StubMw {
            fn name(&self) -> &'static str {
                "stub"
            }
            fn priority(&self) -> u16 {
                100
            }
            async fn before(
                &self,
                _: &str,
                _: serde_json::Value,
                _: &crate::context::Context<serde_json::Value>,
            ) -> Result<Option<serde_json::Value>, ModuleError> {
                Ok(None)
            }
            async fn after(
                &self,
                _: &str,
                _: serde_json::Value,
                _: serde_json::Value,
                _: &crate::context::Context<serde_json::Value>,
            ) -> Result<Option<serde_json::Value>, ModuleError> {
                Ok(None)
            }
            async fn on_error(
                &self,
                _: &str,
                _: serde_json::Value,
                _: &ModuleError,
                _: &crate::context::Context<serde_json::Value>,
            ) -> Result<Option<serde_json::Value>, ModuleError> {
                Ok(None)
            }
        }

        let yaml = r#"
middleware:
  - type: custom
    handler: "stub"
    priority: 950
"#;
        let chain = MiddlewareChainConfig::from_yaml(yaml).unwrap();
        let mut factory = MiddlewareFactory::new();
        factory.register_custom(
            "stub",
            Arc::new(|_cfg| Ok(Box::new(StubMw) as Box<dyn Middleware>)),
        );
        let built = factory.build_chain(&chain).unwrap();
        // Stub's intrinsic priority is 100; the YAML override must win.
        assert_eq!(built[0].priority(), 950);
        assert_eq!(built[0].name(), "stub");
    }
}

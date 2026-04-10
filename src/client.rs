// APCore Protocol — Client
// Spec reference: APCore client entry point

use serde_json::Value;

use crate::config::Config;
use crate::context::Context;
use crate::errors::ModuleError;
use crate::events::emitter::EventEmitter;
use crate::events::subscribers::EventSubscriber;
use crate::executor::Executor;
use crate::middleware::adapters::{AfterMiddleware, BeforeMiddleware};
use crate::middleware::base::Middleware;
use crate::module::ModuleAnnotations;
use crate::registry::registry::{ModuleDescriptor, Registry};

/// Main entry point for interacting with the APCore system.
pub struct APCore {
    pub config: Config,
    executor: Executor,
    event_emitter: Option<EventEmitter>,
}

impl std::fmt::Debug for APCore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("APCore")
            .field("config", &self.config)
            .field("registry", &self.executor.registry)
            .field("event_emitter", &self.event_emitter)
            .finish()
    }
}

impl Default for APCore {
    fn default() -> Self {
        Self::new()
    }
}

impl APCore {
    /// Create a new APCore client with default configuration.
    pub fn new() -> Self {
        let config = Config::default();
        let executor = Executor::new(Registry::new(), config.clone());
        Self {
            config,
            executor,
            event_emitter: None,
        }
    }

    /// Create a new APCore client with all optional parameters.
    ///
    /// If both `executor` and `registry` are provided, the `executor` takes precedence
    /// and the `registry` is ignored (the executor already contains its own registry).
    pub fn with_options(
        registry: Option<Registry>,
        executor: Option<Executor>,
        config: Option<Config>,
        metrics_collector: Option<crate::observability::metrics::MetricsCollector>,
    ) -> Self {
        let config = config.unwrap_or_default();
        let executor =
            executor.unwrap_or_else(|| Executor::new(registry.unwrap_or_default(), config.clone()));
        let _ = metrics_collector; // reserved for future use
        Self {
            config,
            executor,
            event_emitter: None,
        }
    }

    /// Create a new APCore client with the given configuration.
    pub fn with_config(config: Config) -> Self {
        let executor = Executor::new(Registry::new(), config.clone());
        Self {
            config,
            executor,
            event_emitter: None,
        }
    }

    /// Create a new APCore client from a pre-built Registry and Executor.
    ///
    /// Builds an `Executor` from the given `registry` and a default `Config`.
    /// To supply a custom config, use [`with_options`] instead.
    pub fn with_components(registry: Registry, config: Config) -> Self {
        let executor = Executor::new(registry, config.clone());
        Self {
            config,
            executor,
            event_emitter: None,
        }
    }

    /// Create a new APCore client from a configuration file path.
    pub fn from_path(path: impl AsRef<std::path::Path>) -> Result<Self, ModuleError> {
        let config = Config::load(path.as_ref())?;
        Ok(Self::with_config(config))
    }

    /// Call (execute) a module by ID with the given inputs.
    pub async fn call(
        &self,
        module_id: &str,
        inputs: serde_json::Value,
        ctx: Option<&Context<serde_json::Value>>,
        version_hint: Option<&str>,
    ) -> Result<serde_json::Value, ModuleError> {
        self.executor
            .call(module_id, inputs, ctx, version_hint)
            .await
    }

    /// Validate module inputs without executing (spec §12.3).
    ///
    /// Returns a `PreflightResult` with per-check status. `ctx` enables
    /// call-chain checks against real caller state; pass `None` to validate
    /// from an anonymous external context.
    pub async fn validate(
        &self,
        module_id: &str,
        inputs: &serde_json::Value,
        ctx: Option<&crate::context::Context<serde_json::Value>>,
    ) -> Result<crate::module::PreflightResult, ModuleError> {
        self.executor.validate(module_id, inputs, ctx).await
    }

    /// Register a module with the given module_id.
    pub fn register(
        &mut self,
        module_id: &str,
        module: Box<dyn crate::module::Module>,
    ) -> Result<(), ModuleError> {
        let descriptor = ModuleDescriptor {
            name: module_id.to_string(),
            annotations: ModuleAnnotations::default(),
            input_schema: module.input_schema(),
            output_schema: module.output_schema(),
            enabled: true,
            tags: vec![],
            dependencies: vec![],
        };
        std::sync::Arc::get_mut(&mut self.executor.registry)
            .expect("registry not shared yet")
            .register(module_id, module, descriptor)
    }

    /// Trigger module discovery from configured extension directories.
    ///
    /// Returns the number of newly discovered modules, or 0 if no discoverer
    /// is configured on the registry.
    pub async fn discover(&mut self) -> Result<usize, ModuleError> {
        let registry = std::sync::Arc::get_mut(&mut self.executor.registry).ok_or_else(|| {
            ModuleError::new(
                crate::errors::ErrorCode::GeneralInternalError,
                "Cannot discover: registry is shared".to_string(),
            )
        })?;
        match registry.discover_internal().await {
            Ok(names) => Ok(names.len()),
            Err(e) if e.code == crate::errors::ErrorCode::ModuleLoadError => {
                // No discoverer configured — not an error, just nothing to discover.
                Ok(0)
            }
            Err(e) => Err(e),
        }
    }

    /// List all registered modules, optionally filtered by tags and/or prefix.
    pub fn list_modules(&self, tags: Option<&[&str]>, prefix: Option<&str>) -> Vec<String> {
        self.executor
            .registry
            .list(tags, prefix)
            .into_iter()
            .map(|s| s.to_string())
            .collect()
    }

    /// Add a middleware to the execution pipeline.
    ///
    /// Returns an error if the middleware's priority exceeds the allowed range.
    /// Returns `&mut Self` for chaining.
    pub fn use_middleware(
        &mut self,
        middleware: Box<dyn Middleware>,
    ) -> Result<&mut Self, crate::errors::ModuleError> {
        self.executor.use_middleware(middleware)?;
        Ok(self)
    }

    /// Remove a middleware by name.
    pub fn remove(&mut self, name: &str) -> bool {
        self.executor.remove(name)
    }

    /// Get a reference to the registry.
    pub fn registry(&self) -> &Registry {
        &self.executor.registry
    }

    /// Get a reference to the executor.
    pub fn executor(&self) -> &Executor {
        &self.executor
    }

    /// Disable a module. Calls to disabled modules will raise ModuleDisabledError.
    pub fn disable(&mut self, module_id: &str, reason: Option<&str>) -> Result<(), ModuleError> {
        let _ = reason; // reason logging reserved for future use
        let registry = std::sync::Arc::get_mut(&mut self.executor.registry).ok_or_else(|| {
            ModuleError::new(
                crate::errors::ErrorCode::GeneralInternalError,
                format!("Cannot disable '{}': registry is shared", module_id),
            )
        })?;
        registry.disable(module_id)
    }

    /// Re-enable a previously disabled module.
    pub fn enable(&mut self, module_id: &str, reason: Option<&str>) -> Result<(), ModuleError> {
        let _ = reason; // reason logging reserved for future use
        let registry = std::sync::Arc::get_mut(&mut self.executor.registry).ok_or_else(|| {
            ModuleError::new(
                crate::errors::ErrorCode::GeneralInternalError,
                format!("Cannot enable '{}': registry is shared", module_id),
            )
        })?;
        registry.enable(module_id)
    }

    /// Subscribe to an event type. Returns the subscriber ID.
    ///
    /// The `event_type` is bound to the subscriber so it only receives matching
    /// events (glob patterns like `"apcore.*"` are supported).
    /// Lazily initializes the event emitter on first use.
    pub fn on(&mut self, event_type: &str, subscriber: Box<dyn EventSubscriber>) -> String {
        let wrapped = Box::new(EventTypeSubscriber {
            event_type: event_type.to_string(),
            inner: subscriber,
        });
        let emitter = self.event_emitter.get_or_insert_with(EventEmitter::new);
        let id = wrapped.subscriber_id().to_string();
        emitter.subscribe(wrapped);
        id
    }

    /// Unsubscribe by subscriber ID.
    pub fn off(&mut self, subscriber_id: &str) -> bool {
        if let Some(ref mut emitter) = self.event_emitter {
            emitter.unsubscribe_by_id(subscriber_id)
        } else {
            false
        }
    }

    /// Get the event emitter, if configured.
    pub fn events(&self) -> Option<&EventEmitter> {
        self.event_emitter.as_ref()
    }

    /// Reload modules from the configured modules path.
    pub async fn reload(&mut self) -> Result<(), ModuleError> {
        self.config.reload()?;
        // Re-discovery would go here once discoverer is configured
        Ok(())
    }

    /// Shut down the client and release resources.
    pub async fn shutdown(&mut self) -> Result<(), ModuleError> {
        Ok(())
    }

    /// Stream execution of a module.
    pub async fn stream(
        &self,
        module_id: &str,
        inputs: Value,
        ctx: Option<&Context<Value>>,
        version_hint: Option<&str>,
    ) -> Result<Vec<Value>, ModuleError> {
        self.executor
            .stream(module_id, inputs, ctx, version_hint)
            .await
    }

    /// Describe a module by ID.
    pub fn describe(&self, module_id: &str) -> String {
        match self.executor.registry.get(module_id) {
            Some(module) => module.description().to_string(),
            None => format!("Module '{}' not found", module_id),
        }
    }

    /// Add a before callback middleware. Returns `&mut Self` for chaining.
    pub fn use_before(
        &mut self,
        middleware: Box<dyn BeforeMiddleware>,
    ) -> Result<&mut Self, crate::errors::ModuleError> {
        self.executor.use_before(middleware)?;
        Ok(self)
    }

    /// Add an after callback middleware. Returns `&mut Self` for chaining.
    pub fn use_after(
        &mut self,
        middleware: Box<dyn AfterMiddleware>,
    ) -> Result<&mut Self, crate::errors::ModuleError> {
        self.executor.use_after(middleware)?;
        Ok(self)
    }
}

/// Wrapper that binds an `event_type` pattern to an inner subscriber,
/// overriding its `event_pattern()` so the emitter only delivers matching events.
#[derive(Debug)]
struct EventTypeSubscriber {
    event_type: String,
    inner: Box<dyn EventSubscriber>,
}

#[async_trait::async_trait]
impl EventSubscriber for EventTypeSubscriber {
    fn subscriber_id(&self) -> &str {
        self.inner.subscriber_id()
    }

    fn event_pattern(&self) -> &str {
        &self.event_type
    }

    async fn on_event(
        &self,
        event: &crate::events::emitter::ApCoreEvent,
    ) -> Result<(), ModuleError> {
        self.inner.on_event(event).await
    }
}

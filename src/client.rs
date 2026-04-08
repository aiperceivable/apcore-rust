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

    /// Alias for `call()` — provided for spec compatibility.
    pub async fn call_async(
        &self,
        module_id: &str,
        inputs: serde_json::Value,
        ctx: Option<&Context<serde_json::Value>>,
        version_hint: Option<&str>,
    ) -> Result<serde_json::Value, ModuleError> {
        self.call(module_id, inputs, ctx, version_hint).await
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

    /// Trigger module discovery.
    pub async fn discover(&mut self) -> Result<usize, ModuleError> {
        // No discoverer configured by default
        Ok(0)
    }

    /// List all registered modules, optionally filtered by tags and/or prefix.
    pub fn list_modules(&self, tags: Option<&[String]>, prefix: Option<&str>) -> Vec<String> {
        // Convert &[String] to &[&str] for the registry API
        let tag_refs: Vec<&str>;
        let tags_param = match tags {
            Some(t) => {
                tag_refs = t.iter().map(|s| s.as_str()).collect();
                Some(tag_refs.as_slice())
            }
            None => None,
        };
        self.executor
            .registry
            .list(tags_param, prefix)
            .into_iter()
            .map(|s| s.to_string())
            .collect()
    }

    /// Add a middleware to the execution pipeline.
    ///
    /// Returns an error if the middleware's priority exceeds the allowed range.
    pub fn use_middleware(
        &mut self,
        middleware: Box<dyn Middleware>,
    ) -> Result<(), crate::errors::ModuleError> {
        self.executor.use_middleware(middleware)
    }

    /// Remove a middleware by name.
    pub fn remove(&mut self, name: &str) -> bool {
        self.executor.remove(name)
    }

    /// Remove a middleware by name (legacy alias).
    pub fn remove_middleware(&mut self, name: &str) -> bool {
        self.remove(name)
    }

    /// Get a reference to the registry.
    pub fn registry(&self) -> &Registry {
        &self.executor.registry
    }

    /// Get a reference to the executor.
    pub fn executor(&self) -> &Executor {
        &self.executor
    }

    /// Disable a module with an optional reason.
    pub fn disable(&mut self, _module_id: &str, _reason: Option<&str>) -> Result<(), ModuleError> {
        // Needs sys_modules support — no-op for now
        Ok(())
    }

    /// Re-enable a previously disabled module.
    pub fn enable(&mut self, _module_id: &str, _reason: Option<&str>) -> Result<(), ModuleError> {
        // Needs sys_modules support — no-op for now
        Ok(())
    }

    /// Subscribe to an event type. Returns the subscriber ID.
    ///
    /// Lazily initializes the event emitter on first use.
    pub fn on(&mut self, _event_type: &str, subscriber: Box<dyn EventSubscriber>) -> String {
        let emitter = self.event_emitter.get_or_insert_with(EventEmitter::new);
        let id = subscriber.subscriber_id().to_string();
        emitter.subscribe(subscriber);
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

    /// Add a before callback middleware.
    pub fn use_before(
        &mut self,
        middleware: Box<dyn BeforeMiddleware>,
    ) -> Result<(), crate::errors::ModuleError> {
        self.executor.use_before(middleware)
    }

    /// Add an after callback middleware.
    pub fn use_after(
        &mut self,
        middleware: Box<dyn AfterMiddleware>,
    ) -> Result<(), crate::errors::ModuleError> {
        self.executor.use_after(middleware)
    }
}

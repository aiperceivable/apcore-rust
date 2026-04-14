// APCore Protocol — Client
// Spec reference: APCore client entry point

use std::sync::Arc;

use serde_json::Value;

use crate::config::Config;
use crate::context::Context;
use crate::decorator::FunctionModule;
use crate::errors::ModuleError;
use crate::events::emitter::EventEmitter;
use crate::events::subscribers::EventSubscriber;
use crate::executor::Executor;
use crate::middleware::adapters::{AfterMiddleware, BeforeMiddleware};
use crate::middleware::base::Middleware;
use crate::module::ModuleAnnotations;
use crate::observability::metrics::MetricsCollector;
use crate::registry::registry::{ModuleDescriptor, Registry};
use crate::sys_modules::SysModulesContext;

/// Main entry point for interacting with the APCore system.
pub struct APCore {
    pub config: Config,
    executor: Executor,
    /// Single shared `Arc<Registry>` used by the executor, the pipeline, and
    /// all sys modules. Interior mutability on `Registry` removes the need
    /// for `Arc::get_mut` or an external `Mutex`.
    registry: Arc<Registry>,
    event_emitter: Option<EventEmitter>,
    metrics_collector: Option<MetricsCollector>,
    sys_modules_context: Option<SysModulesContext>,
}

impl std::fmt::Debug for APCore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("APCore")
            .field("config", &self.config)
            .field("executor", &"<Executor>")
            .field("registry", &self.registry)
            .field("event_emitter", &self.event_emitter)
            .field("metrics_collector", &self.metrics_collector)
            .field(
                "sys_modules_registered",
                &self.sys_modules_context.is_some(),
            )
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
        Self::with_options(None, None, None, None)
    }

    /// Create a new APCore client with all optional parameters.
    ///
    /// A single `Arc<Registry>` is shared between the executor, the pipeline,
    /// and every built-in sys module — there is exactly one registry
    /// instance per APCore client.
    ///
    /// When `sys_modules.enabled` is true in the config (the default), built-in
    /// system modules are automatically registered into the executor pipeline.
    /// This registration is now fully synchronous and runtime-agnostic because
    /// `Registry` uses `parking_lot::RwLock` for interior mutability.
    ///
    /// **Cross-language note:** Python and TypeScript expose `config_path` /
    /// `configPath` as a 5th constructor parameter. Rust splits this into a
    /// dedicated [`APCore::from_path`] constructor — use `with_options` when
    /// you want to inject explicit components, and `from_path` when you want
    /// to load configuration from a YAML file.
    pub fn with_options(
        registry: Option<Registry>,
        executor: Option<Executor>,
        config: Option<Config>,
        metrics_collector: Option<MetricsCollector>,
    ) -> Self {
        let config = config.unwrap_or_default();

        // Resolve the shared registry: use the executor's if provided,
        // otherwise the explicit `registry` arg, otherwise a fresh default.
        let registry: Arc<Registry> = match executor.as_ref() {
            Some(e) => Arc::clone(&e.registry),
            None => Arc::new(registry.unwrap_or_default()),
        };

        let executor = executor
            .unwrap_or_else(|| Executor::new(Arc::clone(&registry), Arc::new(config.clone())));

        let sys_modules_context = if Self::sys_modules_enabled(&config) {
            crate::sys_modules::register_sys_modules(
                Arc::clone(&registry),
                &executor,
                &config,
                metrics_collector.clone(),
            )
        } else {
            None
        };

        Self {
            config,
            executor,
            registry,
            event_emitter: None,
            metrics_collector,
            sys_modules_context,
        }
    }

    /// Return whether the sys_modules auto-registration is enabled
    /// according to the given config. Defaults to `true` when the key
    /// is absent.
    fn sys_modules_enabled(config: &Config) -> bool {
        config
            .get("sys_modules.enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(true)
    }

    /// Create a new APCore client with the given configuration.
    pub fn with_config(config: Config) -> Self {
        Self::with_options(None, None, Some(config), None)
    }

    /// Create a new APCore client from a pre-built Registry and Executor.
    ///
    /// Builds an `Executor` from the given `registry` and a default `Config`.
    /// To supply a custom config, use [`with_options`] instead.
    pub fn with_components(registry: Registry, config: Config) -> Self {
        Self::with_options(Some(registry), None, Some(config), None)
    }

    /// Create a new APCore client from a configuration file path.
    pub fn from_path(path: impl AsRef<std::path::Path>) -> Result<Self, ModuleError> {
        let config = Config::load(path.as_ref())?;
        Ok(Self::with_config(config))
    }

    /// Register a function as a module — convenience wrapper that creates a
    /// [`FunctionModule`] and registers it in a single call.
    ///
    /// This mirrors the `module()` helper available in the Python and TypeScript
    /// SDKs. The handler closure must be an async function that takes
    /// `(serde_json::Value, &Context<serde_json::Value>)` and returns
    /// `Result<serde_json::Value, ModuleError>`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// client.module(
    ///     "math.add",
    ///     "Add two numbers",
    ///     serde_json::json!({"type": "object"}),
    ///     serde_json::json!({"type": "object"}),
    ///     None,  // documentation
    ///     vec![], // tags
    ///     None,  // version
    ///     None,  // metadata
    ///     vec![], // examples
    ///     |inputs, _ctx| Box::pin(async move { Ok(inputs) }),
    /// )?;
    /// ```
    #[allow(clippy::too_many_arguments)]
    pub fn module<F>(
        &mut self,
        module_id: &str,
        description: &str,
        input_schema: serde_json::Value,
        output_schema: serde_json::Value,
        documentation: Option<String>,
        tags: Vec<String>,
        version: Option<&str>,
        metadata: Option<std::collections::HashMap<String, serde_json::Value>>,
        examples: Vec<crate::module::ModuleExample>,
        handler: F,
    ) -> Result<&mut Self, ModuleError>
    where
        F: for<'a> Fn(
                serde_json::Value,
                &'a Context<serde_json::Value>,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = Result<serde_json::Value, ModuleError>>
                        + Send
                        + 'a,
                >,
            > + Send
            + Sync
            + 'static,
    {
        let resolved_version = version.unwrap_or("0.1.0").to_string();
        let resolved_metadata = metadata.unwrap_or_default();
        let func_module = FunctionModule::with_description(
            ModuleAnnotations::default(),
            input_schema.clone(),
            output_schema.clone(),
            description,
            documentation,
            tags.clone(),
            &resolved_version,
            resolved_metadata,
            examples,
            handler,
        );
        let descriptor = ModuleDescriptor {
            name: module_id.to_string(),
            annotations: ModuleAnnotations::default(),
            input_schema,
            output_schema,
            enabled: true,
            tags,
            dependencies: vec![],
        };
        self.registry
            .register(module_id, Box::new(func_module), descriptor)?;
        Ok(self)
    }

    /// Call (execute) a module by ID with the given inputs.
    ///
    /// `inputs` accepts either a `serde_json::Value` directly or `None`.
    /// When `None` is passed, an empty JSON object `{}` is used, matching
    /// the Python and TypeScript SDKs where inputs can be `None`/`undefined`.
    ///
    /// In Rust, `call()` is already async — there is no separate sync variant.
    /// This method is the equivalent of both `call()` and `call_async()` in the
    /// Python and TypeScript SDKs.
    #[doc(alias = "call_async")]
    pub async fn call(
        &self,
        module_id: &str,
        inputs: impl Into<Option<serde_json::Value>>,
        ctx: Option<&Context<serde_json::Value>>,
        version_hint: Option<&str>,
    ) -> Result<serde_json::Value, ModuleError> {
        let resolved_inputs = inputs.into().unwrap_or_else(|| serde_json::json!({}));
        self.executor
            .call(module_id, resolved_inputs, ctx, version_hint)
            .await
    }

    /// Validate module inputs without executing (spec §12.3).
    ///
    /// `inputs` is optional -- pass `None` to validate with an empty `{}`
    /// object, matching the Python and TypeScript SDKs where inputs can be
    /// `None`/`undefined`. A `&serde_json::Value` reference is also accepted
    /// directly for backward compatibility.
    ///
    /// Returns a `PreflightResult` with per-check status. `ctx` enables
    /// call-chain checks against real caller state; pass `None` to validate
    /// from an anonymous external context.
    pub async fn validate<'v>(
        &self,
        module_id: &str,
        inputs: impl Into<Option<&'v serde_json::Value>>,
        ctx: Option<&crate::context::Context<serde_json::Value>>,
    ) -> Result<crate::module::PreflightResult, ModuleError> {
        let empty = serde_json::json!({});
        let resolved_inputs = inputs.into().unwrap_or(&empty);
        self.executor
            .validate(module_id, resolved_inputs, ctx)
            .await
    }

    /// Register a module with the given module_id.
    pub fn register(
        &self,
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
        self.registry.register(module_id, module, descriptor)
    }

    /// Trigger module discovery from configured extension directories.
    ///
    /// Returns the number of newly discovered modules, or 0 if no discoverer
    /// is configured on the registry.
    pub async fn discover(&self) -> Result<usize, ModuleError> {
        match self.registry.discover_internal().await {
            Ok(count) => Ok(count),
            Err(e) if e.code == crate::errors::ErrorCode::ModuleLoadError => {
                // No discoverer configured — not an error, just nothing to discover.
                Ok(0)
            }
            Err(e) => Err(e),
        }
    }

    /// List all registered modules, optionally filtered by tags and/or prefix.
    pub fn list_modules(&self, tags: Option<&[&str]>, prefix: Option<&str>) -> Vec<String> {
        self.registry.list(tags, prefix)
    }

    /// Register a middleware in the execution pipeline.
    ///
    /// Returns an error if the middleware's priority exceeds the allowed range.
    /// Returns `&Self` for chaining. Takes `&self` — the underlying
    /// `MiddlewareManager` uses interior mutability, so middleware can be
    /// added after `APCore` has been shared behind an `Arc` or a shared
    /// reference.
    ///
    /// **Cross-language note:** The Python and TypeScript SDKs expose this
    /// method as `use()`. Rust names it `use_middleware` because `use` is a
    /// reserved keyword.
    pub fn use_middleware(
        &self,
        middleware: Box<dyn Middleware>,
    ) -> Result<&Self, crate::errors::ModuleError> {
        self.executor.use_middleware(middleware)?;
        Ok(self)
    }

    /// Remove a middleware by its name string.
    ///
    /// This is a Rust-specific convenience that accepts a `&str` directly.
    /// For an API closer to Python/TypeScript (which accept the middleware
    /// object), see [`remove_middleware`](Self::remove_middleware).
    pub fn remove(&self, name: &str) -> bool {
        self.executor.remove(name)
    }

    /// Remove a middleware by reference, extracting its name via
    /// [`Middleware::name()`].
    ///
    /// This mirrors the Python and TypeScript `remove(middleware)` API,
    /// which accept the middleware object directly. In Rust, since trait
    /// objects do not support identity comparison, the middleware is matched
    /// by its [`name()`](Middleware::name) return value.
    pub fn remove_middleware(&self, middleware: &dyn Middleware) -> bool {
        self.executor.remove(middleware.name())
    }

    /// Get a reference to the registry.
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// Get the shared registry as an `Arc<Registry>` (for handing out to
    /// components that need to share ownership).
    pub fn registry_arc(&self) -> Arc<Registry> {
        Arc::clone(&self.registry)
    }

    /// Get a reference to the executor.
    pub fn executor(&self) -> &Executor {
        &self.executor
    }

    /// Disable a module by routing through the executor pipeline.
    ///
    /// This calls `system.control.toggle_feature` through the executor,
    /// matching the Python and TypeScript SDK behavior (events, ACL, and
    /// middleware are applied).
    ///
    /// **Cross-language note:** Python and TypeScript SDKs accept a `reason`
    /// string with a default value (e.g. `reason=""` or `reason=None`). In Rust,
    /// `reason` is `Option<&str>` — pass `None` to use the default message, or
    /// `Some("my reason")` to provide a custom one.
    pub async fn disable(
        &self,
        module_id: &str,
        reason: Option<&str>,
    ) -> Result<Value, ModuleError> {
        self.executor
            .call(
                "system.control.toggle_feature",
                serde_json::json!({
                    "module_id": module_id,
                    "enabled": false,
                    "reason": reason.unwrap_or("Disabled via APCore client")
                }),
                None,
                None,
            )
            .await
    }

    /// Re-enable a previously disabled module by routing through the executor pipeline.
    ///
    /// This calls `system.control.toggle_feature` through the executor,
    /// matching the Python and TypeScript SDK behavior (events, ACL, and
    /// middleware are applied).
    ///
    /// **Cross-language note:** Python and TypeScript SDKs accept a `reason`
    /// string with a default value (e.g. `reason=""` or `reason=None`). In Rust,
    /// `reason` is `Option<&str>` — pass `None` to use the default message, or
    /// `Some("my reason")` to provide a custom one.
    pub async fn enable(
        &self,
        module_id: &str,
        reason: Option<&str>,
    ) -> Result<Value, ModuleError> {
        self.executor
            .call(
                "system.control.toggle_feature",
                serde_json::json!({
                    "module_id": module_id,
                    "enabled": true,
                    "reason": reason.unwrap_or("Enabled via APCore client")
                }),
                None,
                None,
            )
            .await
    }

    /// **Preferred API:** Subscribe to an event type using a closure.
    ///
    /// Convenience wrapper around [`on()`](Self::on) that accepts a plain
    /// function or closure instead of requiring a boxed [`EventSubscriber`].
    /// The closure receives each matching [`ApCoreEvent`](crate::events::emitter::ApCoreEvent)
    /// by reference.
    ///
    /// Returns the auto-generated subscriber ID (a UUID string).
    ///
    /// **Cross-language note:** Python and TypeScript SDKs register event
    /// listeners with a plain callback closure and return the subscriber object.
    /// `on_fn()` is the idiomatic Rust equivalent — it wraps the closure in an
    /// internal `ClosureSubscriber` and returns the subscriber ID string,
    /// which can be passed to [`off()`](Self::off) to unsubscribe.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let id = client.on_fn("apcore.*", |event| {
    ///     println!("Got event: {}", event.event_type);
    /// });
    /// client.off(&id); // unsubscribe by ID
    /// ```
    pub fn on_fn(
        &mut self,
        event_type: &str,
        callback: impl Fn(&crate::events::emitter::ApCoreEvent) + Send + Sync + 'static,
    ) -> String {
        let subscriber = Box::new(ClosureSubscriber {
            id: uuid::Uuid::new_v4().to_string(),
            callback: Box::new(callback),
        });
        self.on(event_type, subscriber)
    }

    /// Subscribe to an event type using a boxed [`EventSubscriber`]. Returns the subscriber ID.
    ///
    /// The `event_type` is bound to the subscriber so it only receives matching
    /// events (glob patterns like `"apcore.*"` are supported).
    /// Lazily initializes the event emitter on first use.
    ///
    /// **Cross-language note:** Python and TypeScript SDKs accept a plain
    /// callback closure here. In Rust, prefer [`on_fn()`](Self::on_fn) for
    /// closure-based subscriptions; use `on()` only when you need a custom
    /// [`EventSubscriber`] implementation.
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

    /// Reload configuration from disk.
    ///
    /// **Rust-specific:** This method is not available in Python or TypeScript
    /// SDKs. It reloads the `Config` object; module re-discovery will be added
    /// once the discoverer component is configured.
    #[allow(clippy::unused_async)] // API stub for cross-language parity with Python/TypeScript SDKs
    pub async fn reload(&mut self) -> Result<(), ModuleError> {
        self.config.reload()?;
        Ok(())
    }

    /// Stream execution of a module.
    ///
    /// `inputs` accepts either a `serde_json::Value` directly or `None`.
    /// When `None` is passed, an empty JSON object `{}` is used, matching
    /// the Python and TypeScript SDKs where inputs can be `None`/`undefined`.
    ///
    /// Returns an async `Stream` of chunks. Each chunk is delivered to the
    /// caller as soon as it is produced by the underlying module -- true
    /// incremental streaming, no buffering. Phase 3 validation runs after
    /// the inner stream is exhausted; if it fails, the error is yielded as
    /// the final item.
    pub fn stream<'a>(
        &'a self,
        module_id: &str,
        inputs: impl Into<Option<Value>>,
        ctx: Option<&Context<Value>>,
        version_hint: Option<&str>,
    ) -> std::pin::Pin<Box<dyn futures_core::Stream<Item = Result<Value, ModuleError>> + Send + 'a>>
    {
        let resolved_inputs = inputs.into().unwrap_or_else(|| serde_json::json!({}));
        self.executor
            .stream(module_id, resolved_inputs, ctx, version_hint)
    }

    /// Describe a module by ID.
    pub fn describe(&self, module_id: &str) -> String {
        match self.registry.get(module_id) {
            Some(module) => module.description().to_string(),
            None => format!("Module '{module_id}' not found"),
        }
    }

    /// Add a before callback middleware. Returns `&Self` for chaining.
    pub fn use_before(
        &self,
        middleware: Box<dyn BeforeMiddleware>,
    ) -> Result<&Self, crate::errors::ModuleError> {
        self.executor.use_before(middleware)?;
        Ok(self)
    }

    /// Add an after callback middleware. Returns `&Self` for chaining.
    pub fn use_after(
        &self,
        middleware: Box<dyn AfterMiddleware>,
    ) -> Result<&Self, crate::errors::ModuleError> {
        self.executor.use_after(middleware)?;
        Ok(self)
    }
}

/// Internal subscriber that wraps a plain closure for [`APCore::on_fn()`].
struct ClosureSubscriber {
    id: String,
    callback: Box<dyn Fn(&crate::events::emitter::ApCoreEvent) + Send + Sync>,
}

impl std::fmt::Debug for ClosureSubscriber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClosureSubscriber")
            .field("id", &self.id)
            .field("callback", &"<closure>")
            .finish()
    }
}

#[async_trait::async_trait]
impl EventSubscriber for ClosureSubscriber {
    fn subscriber_id(&self) -> &str {
        &self.id
    }

    fn event_pattern(&self) -> &'static str {
        "*"
    }

    async fn on_event(
        &self,
        event: &crate::events::emitter::ApCoreEvent,
    ) -> Result<(), ModuleError> {
        (self.callback)(event);
        Ok(())
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

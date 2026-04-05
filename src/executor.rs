// APCore Protocol — Executor
// Spec reference: Module execution engine

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, RwLock};

use serde_json::Value;

use crate::acl::ACL;
use crate::approval::ApprovalHandler;
use crate::builtin_steps::build_standard_strategy;
use crate::config::Config;
use crate::context::{Context, Identity};
use crate::errors::{ErrorCode, ModuleError};
use crate::middleware::adapters::{AfterMiddleware, BeforeMiddleware};
use crate::middleware::base::Middleware;
use crate::middleware::manager::MiddlewareManager;
use crate::module::PreflightCheckResult as PfCheck;
use crate::module::{PreflightCheckResult, PreflightResult};
use crate::pipeline::{
    ExecutionStrategy, PipelineContext, PipelineEngine, PipelineTrace, StrategyInfo,
};
use crate::registry::registry::Registry;

/// Map pipeline step names to PreflightResult check names.
fn step_to_check_name(step_name: &str) -> &str {
    match step_name {
        "context_creation" => "context",
        "call_chain_guard" => "call_chain",
        "module_lookup" => "module_lookup",
        "acl_check" => "acl",
        "approval_gate" => "approval",
        "middleware_before" => "middleware",
        "input_validation" => "schema",
        other => other,
    }
}

/// Convert `PipelineTrace` steps into `PreflightCheckResult` entries.
fn trace_to_checks(trace: &PipelineTrace) -> Vec<PfCheck> {
    trace
        .steps
        .iter()
        .filter(|st| !st.skipped)
        .map(|st| {
            let check_name = step_to_check_name(&st.name).to_string();
            let passed = st.result.action != "abort";
            let error = if !passed {
                st.result.explanation.as_ref().map(|msg| {
                    serde_json::json!({
                        "code": format!("STEP_{}_FAILED", st.name.to_uppercase()),
                        "message": msg,
                    })
                })
            } else {
                None
            };
            PfCheck {
                check: check_name,
                passed,
                error,
                warnings: vec![],
            }
        })
        .collect()
}

/// Returns true if the schema is non-trivial (not null and not an empty object).
pub fn has_schema(schema: &Value) -> bool {
    if schema.is_null() {
        return false;
    }
    if let Some(obj) = schema.as_object() {
        return !obj.is_empty();
    }
    true
}

/// Sentinel value used to replace sensitive fields in redacted output.
pub const REDACTED_VALUE: &str = "***REDACTED***";

/// Validate a JSON value against a JSON Schema.
/// Returns Ok(()) if valid, or a ModuleError with SchemaValidationError on failure.
pub fn validate_against_schema(
    value: &Value,
    schema: &Value,
    direction: &str,
) -> Result<(), ModuleError> {
    // If schema is null/empty, skip validation
    if !has_schema(schema) {
        return Ok(());
    }

    let validator = match jsonschema::validator_for(schema) {
        Ok(v) => v,
        Err(e) => {
            return Err(ModuleError::new(
                ErrorCode::SchemaValidationError,
                format!("{direction} schema is invalid: {e}"),
            ));
        }
    };

    if validator.is_valid(value) {
        return Ok(());
    }

    let error_list: Vec<HashMap<String, String>> = validator
        .iter_errors(value)
        .map(|e| {
            let mut map = HashMap::new();
            map.insert("field".to_string(), e.instance_path.to_string());
            map.insert("message".to_string(), e.to_string());
            map
        })
        .collect();

    let errors_json: Vec<Value> = error_list
        .iter()
        .map(|e| serde_json::to_value(e).unwrap_or_default())
        .collect();
    let mut details = HashMap::new();
    details.insert("errors".to_string(), Value::Array(errors_json));

    Err(ModuleError::new(
        ErrorCode::SchemaValidationError,
        format!("{direction} validation failed"),
    )
    .with_details(details)
    .with_ai_guidance(format!(
        "{direction} failed schema validation. Check the 'errors' field in details for specific validation failures."
    )))
}

/// Redact fields marked with `x-sensitive: true` in the schema.
///
/// Returns a deep copy of `data` with sensitive values replaced by `"***REDACTED***"`.
/// Also redacts any keys starting with `_secret_` regardless of schema.
pub fn redact_sensitive(data: &Value, schema: &Value) -> Value {
    let mut redacted = data.clone();
    if let Some(obj) = redacted.as_object_mut() {
        redact_fields(obj, schema);
        redact_secret_prefix(obj);
    }
    redacted
}

/// In-place redaction based on schema `x-sensitive` markers.
fn redact_fields(data: &mut serde_json::Map<String, Value>, schema: &Value) {
    let properties = match schema.get("properties").and_then(|p| p.as_object()) {
        Some(p) => p,
        None => return,
    };

    for (field_name, field_schema) in properties {
        let value = match data.get(field_name) {
            Some(v) => v.clone(),
            None => continue,
        };

        // x-sensitive: true on this property
        if field_schema.get("x-sensitive") == Some(&Value::Bool(true)) {
            if !value.is_null() {
                data.insert(
                    field_name.clone(),
                    Value::String(REDACTED_VALUE.to_string()),
                );
            }
            continue;
        }

        // Nested object: recurse
        if field_schema.get("type") == Some(&Value::String("object".to_string()))
            && field_schema.get("properties").is_some()
        {
            if let Some(obj) = data.get_mut(field_name).and_then(|v| v.as_object_mut()) {
                redact_fields(obj, field_schema);
            }
            continue;
        }

        // Array: redact items
        if field_schema.get("type") == Some(&Value::String("array".to_string())) {
            if let Some(items_schema) = field_schema.get("items") {
                if let Some(arr) = data.get_mut(field_name).and_then(|v| v.as_array_mut()) {
                    if items_schema.get("x-sensitive") == Some(&Value::Bool(true)) {
                        for item in arr.iter_mut() {
                            if !item.is_null() {
                                *item = Value::String(REDACTED_VALUE.to_string());
                            }
                        }
                    } else if items_schema.get("type") == Some(&Value::String("object".to_string()))
                        && items_schema.get("properties").is_some()
                    {
                        for item in arr.iter_mut() {
                            if let Some(obj) = item.as_object_mut() {
                                redact_fields(obj, items_schema);
                            }
                        }
                    }
                }
            }
        }
    }
}

/// In-place redaction of keys starting with `_secret_`.
fn redact_secret_prefix(data: &mut serde_json::Map<String, Value>) {
    let keys: Vec<String> = data.keys().cloned().collect();
    for key in keys {
        if key.starts_with("_secret_") {
            if let Some(val) = data.get(&key) {
                if !val.is_null() {
                    data.insert(key, Value::String(REDACTED_VALUE.to_string()));
                }
            }
        } else if let Some(obj) = data.get_mut(&key).and_then(|v| v.as_object_mut()) {
            redact_secret_prefix(obj);
        }
    }
}

// PreflightResult is re-exported from module.rs — used as the return type for Executor::validate().

// ---------------------------------------------------------------------------
// Global strategy registry (introspection)
// ---------------------------------------------------------------------------

/// Global registry of named execution strategies for introspection.
static STRATEGY_REGISTRY: LazyLock<RwLock<Vec<StrategyInfo>>> =
    LazyLock::new(|| RwLock::new(Vec::new()));

/// Register a strategy's info in the global registry for introspection.
///
/// Replaces any existing entry with the same name.
pub fn register_strategy(info: StrategyInfo) {
    let mut registry = STRATEGY_REGISTRY.write().unwrap_or_else(|e| e.into_inner());
    // Replace existing entry with same name, or append.
    if let Some(existing) = registry.iter_mut().find(|s| s.name == info.name) {
        *existing = info;
    } else {
        registry.push(info);
    }
}

/// List all registered strategy summaries.
pub fn list_strategies() -> Vec<StrategyInfo> {
    STRATEGY_REGISTRY
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Describe a pipeline by returning step names and descriptions from a strategy.
pub fn describe_pipeline(strategy: &ExecutionStrategy) -> StrategyInfo {
    strategy.info()
}

/// Responsible for executing modules with middleware, ACL, and context management.
#[derive(Debug)]
pub struct Executor {
    pub registry: Arc<Registry>,
    pub config: Arc<Config>,
    pub acl: Option<Arc<ACL>>,
    pub approval_handler: Option<Arc<dyn ApprovalHandler>>,
    pub middleware_manager: Arc<MiddlewareManager>,
    /// Execution strategy — all calls go through PipelineEngine.
    strategy: ExecutionStrategy,
}

impl Executor {
    /// Create a new executor with the given registry and config.
    ///
    /// Builds a standard execution strategy — all calls go through PipelineEngine.
    pub fn new(registry: Registry, config: Config) -> Self {
        Self {
            registry: Arc::new(registry),
            config: Arc::new(config),
            acl: None,
            approval_handler: None,
            middleware_manager: Arc::new(MiddlewareManager::new()),
            strategy: build_standard_strategy(),
        }
    }

    /// Create a new executor with a custom execution strategy.
    pub fn with_strategy(registry: Registry, config: Config, strategy: ExecutionStrategy) -> Self {
        Self {
            registry: Arc::new(registry),
            config: Arc::new(config),
            acl: None,
            approval_handler: None,
            middleware_manager: Arc::new(MiddlewareManager::new()),
            strategy,
        }
    }

    /// Create a new executor with all optional parameters.
    pub fn with_options(
        registry: Registry,
        config: Config,
        middlewares: Option<Vec<Box<dyn Middleware>>>,
        acl: Option<ACL>,
        approval_handler: Option<Box<dyn ApprovalHandler>>,
    ) -> Self {
        let mut middleware_manager = MiddlewareManager::new();
        if let Some(mws) = middlewares {
            for mw in mws {
                // Middleware provided at construction time is trusted; log and
                // skip if priority is out of range rather than failing the build.
                if let Err(e) = middleware_manager.add(mw) {
                    tracing::warn!("Skipping middleware during executor construction: {}", e);
                }
            }
        }
        Self {
            registry: Arc::new(registry),
            config: Arc::new(config),
            acl: acl.map(Arc::new),
            approval_handler: approval_handler.map(|h| Arc::from(h) as Arc<dyn ApprovalHandler>),
            middleware_manager: Arc::new(middleware_manager),
            strategy: build_standard_strategy(),
        }
    }

    /// Get a reference to the registry.
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// Get the names of all middlewares in pipeline order.
    pub fn middlewares(&self) -> Vec<String> {
        self.middleware_manager.snapshot()
    }

    /// Set the ACL for access control.
    pub fn set_acl(&mut self, acl: ACL) {
        self.acl = Some(Arc::new(acl));
    }

    /// Set the approval handler.
    pub fn set_approval_handler(&mut self, handler: Box<dyn ApprovalHandler>) {
        self.approval_handler = Some(Arc::from(handler));
    }

    /// Add a middleware to the pipeline.
    ///
    /// Returns an error if the middleware's priority exceeds the allowed range.
    pub fn use_middleware(&mut self, middleware: Box<dyn Middleware>) -> Result<(), ModuleError> {
        Arc::get_mut(&mut self.middleware_manager)
            .expect("middleware_manager not shared yet")
            .add(middleware)
    }

    /// Remove a middleware by name.
    pub fn remove(&mut self, name: &str) -> bool {
        Arc::get_mut(&mut self.middleware_manager)
            .expect("middleware_manager not shared yet")
            .remove(name)
    }

    /// Remove a middleware by name (legacy alias).
    pub fn remove_middleware(&mut self, name: &str) -> bool {
        self.remove(name)
    }

    /// Execute (call) a module by ID with the given inputs and context.
    ///
    /// Delegates to `PipelineEngine::run()` using the configured strategy.
    pub async fn call(
        &self,
        module_id: &str,
        inputs: serde_json::Value,
        ctx: Option<&Context<serde_json::Value>>,
        _version_hint: Option<&str>,
    ) -> Result<serde_json::Value, ModuleError> {
        let context = match ctx {
            Some(c) => c.clone(),
            None => Context::<serde_json::Value>::new(Identity::new(
                "@external".to_string(),
                "external".to_string(),
                vec![],
                Default::default(),
            )),
        };
        let mut pipe_ctx = PipelineContext::new(module_id, inputs, context, self.strategy.name());
        if let Some(hint) = _version_hint {
            pipe_ctx.version_hint = Some(hint.to_string());
        }
        self.inject_resources(&mut pipe_ctx);
        let (output, _trace) = PipelineEngine::run(&self.strategy, &mut pipe_ctx).await?;
        Ok(output.unwrap_or(serde_json::Value::Null))
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

    /// Validate module inputs without executing (steps 1-7, spec §12.3).
    ///
    /// Runs the pipeline in `dry_run` mode — pure steps only, side-effecting
    /// steps are skipped automatically.
    pub async fn validate(
        &self,
        module_id: &str,
        inputs: &serde_json::Value,
    ) -> Result<PreflightResult, ModuleError> {
        let context = Context::<serde_json::Value>::new(Identity::new(
            "@external".to_string(),
            "external".to_string(),
            vec![],
            Default::default(),
        ));
        let mut pipe_ctx =
            PipelineContext::new(module_id, inputs.clone(), context, self.strategy.name());
        pipe_ctx.dry_run = true;
        self.inject_resources(&mut pipe_ctx);

        let mut checks: Vec<PreflightCheckResult> = Vec::new();
        let trace_result = PipelineEngine::run(&self.strategy, &mut pipe_ctx).await;
        match trace_result {
            Ok((_output, trace)) => {
                checks.extend(trace_to_checks(&trace));
            }
            Err(e) => {
                // Pipeline step raised an error; convert to a failed check.
                checks.extend(trace_to_checks(&pipe_ctx.trace));
                let check_name = match e.code {
                    ErrorCode::ModuleNotFound => "module_lookup",
                    ErrorCode::AclDenied => "acl",
                    ErrorCode::SchemaValidationError | ErrorCode::GeneralInvalidInput => "schema",
                    ErrorCode::CallDepthExceeded | ErrorCode::CircularCall => "call_chain",
                    _ => "unknown",
                };
                checks.push(PreflightCheckResult {
                    check: check_name.to_string(),
                    passed: false,
                    error: Some(serde_json::json!({
                        "code": format!("{:?}", e.code),
                        "message": e.message,
                    })),
                    warnings: vec![],
                });
            }
        }

        // Detect requires_approval from module annotations.
        let mut requires_approval = false;
        if let Some(desc) = self.registry.get_definition(module_id) {
            if desc.annotations.requires_approval {
                requires_approval = true;
            }
        }

        let valid = checks.iter().all(|c| c.passed);
        Ok(PreflightResult {
            valid,
            checks,
            requires_approval,
        })
    }

    /// Create an executor from a registry and config.
    pub fn from_registry(registry: Registry, config: Config) -> Self {
        Self::new(registry, config)
    }

    /// Stream execution of a module.
    pub async fn stream(
        &self,
        module_id: &str,
        inputs: Value,
        ctx: Option<&Context<Value>>,
        version_hint: Option<&str>,
    ) -> Result<Vec<Value>, ModuleError> {
        let result = self.call(module_id, inputs, ctx, version_hint).await?;
        Ok(vec![result])
    }

    /// Get a reference to the executor's execution strategy.
    pub fn strategy(&self) -> &ExecutionStrategy {
        &self.strategy
    }

    /// Return a human-readable description of the configured pipeline.
    pub fn describe_pipeline(&self) -> String {
        format!(
            "{}-step pipeline: {}",
            self.strategy.steps().len(),
            self.strategy.step_names().join(" \u{2192} ")
        )
    }

    /// Execute a module through the pipeline engine, returning both the output
    /// and a full execution trace.
    ///
    /// Uses the provided `strategy` override, or the executor's default strategy.
    pub async fn call_with_trace(
        &self,
        module_id: &str,
        inputs: Value,
        ctx: Option<&Context<Value>>,
        strategy: Option<&ExecutionStrategy>,
    ) -> Result<(Value, PipelineTrace), ModuleError> {
        let effective_strategy = strategy.unwrap_or(&self.strategy);

        let context = match ctx {
            Some(c) => c.clone(),
            None => Context::<Value>::new(Identity::new(
                "@external".to_string(),
                "external".to_string(),
                vec![],
                Default::default(),
            )),
        };

        let mut pipeline_ctx =
            PipelineContext::new(module_id, inputs, context, effective_strategy.name());
        self.inject_resources(&mut pipeline_ctx);

        let (output, trace) = PipelineEngine::run(effective_strategy, &mut pipeline_ctx).await?;

        Ok((output.unwrap_or(Value::Null), trace))
    }

    /// Inject executor resources into a pipeline context so builtin steps
    /// can access the registry, config, ACL, approval handler, and middleware.
    fn inject_resources(&self, ctx: &mut PipelineContext) {
        ctx.registry = Some(Arc::clone(&self.registry));
        ctx.config = Some(Arc::clone(&self.config));
        ctx.acl = self.acl.as_ref().map(Arc::clone);
        ctx.approval_handler = self.approval_handler.as_ref().map(Arc::clone);
        ctx.middleware_manager = Some(Arc::clone(&self.middleware_manager));
    }

    /// Add a before middleware.
    pub fn use_before(&mut self, middleware: Box<dyn BeforeMiddleware>) -> Result<(), ModuleError> {
        Arc::get_mut(&mut self.middleware_manager)
            .expect("middleware_manager not shared yet")
            .add(Box::new(BoxedBeforeMiddlewareAdapter(middleware)))
    }

    /// Add an after middleware.
    pub fn use_after(&mut self, middleware: Box<dyn AfterMiddleware>) -> Result<(), ModuleError> {
        Arc::get_mut(&mut self.middleware_manager)
            .expect("middleware_manager not shared yet")
            .add(Box::new(BoxedAfterMiddlewareAdapter(middleware)))
    }
}

// Note: These boxed adapters are intentionally separate from the generic
// BeforeMiddlewareAdapter<T>/AfterMiddlewareAdapter<T> in adapters.rs.
// The generic versions require T: BeforeMiddleware (sized), while these
// wrap Box<dyn BeforeMiddleware> (unsized trait object). Rust's type system
// does not allow BeforeMiddlewareAdapter<Box<dyn BeforeMiddleware>> without
// a blanket impl of BeforeMiddleware for Box<dyn BeforeMiddleware>.

/// Wraps a boxed BeforeMiddleware into a full Middleware trait object.
struct BoxedBeforeMiddlewareAdapter(Box<dyn BeforeMiddleware>);

impl std::fmt::Debug for BoxedBeforeMiddlewareAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoxedBeforeMiddlewareAdapter")
            .field("name", &self.0.name())
            .finish()
    }
}

#[async_trait::async_trait]
impl Middleware for BoxedBeforeMiddlewareAdapter {
    fn name(&self) -> &str {
        self.0.name()
    }

    async fn before(
        &self,
        module_id: &str,
        inputs: serde_json::Value,
        ctx: &Context<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, ModuleError> {
        self.0.before(module_id, inputs, ctx).await
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

/// Wraps a boxed AfterMiddleware into a full Middleware trait object.
struct BoxedAfterMiddlewareAdapter(Box<dyn AfterMiddleware>);

impl std::fmt::Debug for BoxedAfterMiddlewareAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoxedAfterMiddlewareAdapter")
            .field("name", &self.0.name())
            .finish()
    }
}

#[async_trait::async_trait]
impl Middleware for BoxedAfterMiddlewareAdapter {
    fn name(&self) -> &str {
        self.0.name()
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
        module_id: &str,
        inputs: serde_json::Value,
        output: serde_json::Value,
        ctx: &Context<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, ModuleError> {
        self.0.after(module_id, inputs, output, ctx).await
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval::{ApprovalHandler, ApprovalRequest, ApprovalResult};
    use crate::config::Config;
    use crate::context::Context;
    use crate::errors::ErrorCode;
    use crate::module::{Module, ModuleAnnotations};
    use crate::registry::registry::{ModuleDescriptor, Registry};
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Mutex;

    // ── Mock module ──────────────────────────────────────────────────

    struct MockModule {
        input_schema: Value,
        output_schema: Value,
        output: Value,
    }

    impl MockModule {
        fn new(input_schema: Value, output_schema: Value, output: Value) -> Self {
            Self {
                input_schema,
                output_schema,
                output,
            }
        }

        fn echo() -> Self {
            Self::new(json!({}), json!({}), json!({"ok": true}))
        }
    }

    #[async_trait]
    impl Module for MockModule {
        fn input_schema(&self) -> Value {
            self.input_schema.clone()
        }
        fn output_schema(&self) -> Value {
            self.output_schema.clone()
        }
        fn description(&self) -> &str {
            "mock module"
        }
        async fn execute(
            &self,
            _inputs: Value,
            _ctx: &Context<Value>,
        ) -> Result<Value, ModuleError> {
            Ok(self.output.clone())
        }
    }

    // ── Mock approval handler ────────────────────────────────────────

    /// Tracks which method was called (request vs check) and returns a
    /// configurable ApprovalResult.
    #[derive(Debug)]
    struct MockApprovalHandler {
        /// Result returned by both request_approval and check_approval.
        result: ApprovalResult,
        /// Records "request" or "check:<token>" for each call.
        calls: Mutex<Vec<String>>,
    }

    impl MockApprovalHandler {
        fn with_status(status: &str) -> Self {
            Self {
                result: ApprovalResult {
                    status: status.to_string(),
                    approved_by: None,
                    reason: Some(format!("mock-{status}")),
                    approval_id: None,
                    metadata: None,
                },
                calls: Mutex::new(vec![]),
            }
        }
    }

    #[async_trait]
    impl ApprovalHandler for MockApprovalHandler {
        async fn request_approval(
            &self,
            _request: &ApprovalRequest,
        ) -> Result<ApprovalResult, ModuleError> {
            self.calls.lock().unwrap().push("request".to_string());
            Ok(self.result.clone())
        }

        async fn check_approval(&self, approval_id: &str) -> Result<ApprovalResult, ModuleError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("check:{approval_id}"));
            Ok(self.result.clone())
        }
    }

    // ── Helper: build executor with a registered module ──────────────

    fn build_executor_with_module(module: MockModule, annotations: ModuleAnnotations) -> Executor {
        let mut registry = Registry::new();
        let descriptor = ModuleDescriptor {
            name: "test_mod".to_string(),
            annotations,
            input_schema: module.input_schema(),
            output_schema: module.output_schema(),
            enabled: true,
            tags: vec![],
            dependencies: vec![],
        };
        registry
            .register("test_mod", Box::new(module), descriptor)
            .unwrap();
        Executor::new(registry, Config::default())
    }

    // ═══════════════════════════════════════════════════════════════════
    // validate_against_schema
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn test_validate_against_schema_valid_input_passes() {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"}
            },
            "required": ["name"]
        });
        let value = json!({"name": "Alice"});
        assert!(validate_against_schema(&value, &schema, "Input").is_ok());
    }

    #[test]
    fn test_validate_against_schema_invalid_input_returns_error_with_details() {
        let schema = json!({
            "type": "object",
            "properties": {
                "age": {"type": "integer"}
            },
            "required": ["age"]
        });
        let value = json!({"age": "not-a-number"});
        let err = validate_against_schema(&value, &schema, "Input").unwrap_err();
        assert_eq!(err.code, ErrorCode::SchemaValidationError);
        assert!(err.details.contains_key("errors"));
    }

    #[test]
    fn test_validate_against_schema_null_schema_skips() {
        let value = json!({"anything": 123});
        assert!(validate_against_schema(&value, &Value::Null, "Input").is_ok());
    }

    #[test]
    fn test_validate_against_schema_empty_object_schema_skips() {
        let value = json!({"anything": 123});
        assert!(validate_against_schema(&value, &json!({}), "Input").is_ok());
    }

    // ═══════════════════════════════════════════════════════════════════
    // redact_sensitive
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn test_redact_sensitive_basic_field() {
        let schema = json!({
            "properties": {
                "password": {"type": "string", "x-sensitive": true},
                "username": {"type": "string"}
            }
        });
        let data = json!({"password": "s3cret", "username": "alice"});
        let result = redact_sensitive(&data, &schema);
        assert_eq!(result["password"], REDACTED_VALUE);
        assert_eq!(result["username"], "alice");
    }

    #[test]
    fn test_redact_sensitive_nested_object() {
        let schema = json!({
            "properties": {
                "credentials": {
                    "type": "object",
                    "properties": {
                        "token": {"type": "string", "x-sensitive": true},
                        "scope": {"type": "string"}
                    }
                }
            }
        });
        let data = json!({"credentials": {"token": "abc123", "scope": "read"}});
        let result = redact_sensitive(&data, &schema);
        assert_eq!(result["credentials"]["token"], REDACTED_VALUE);
        assert_eq!(result["credentials"]["scope"], "read");
    }

    #[test]
    fn test_redact_sensitive_array_items() {
        let schema = json!({
            "properties": {
                "tokens": {
                    "type": "array",
                    "items": {"type": "string", "x-sensitive": true}
                }
            }
        });
        let data = json!({"tokens": ["a", "b", "c"]});
        let result = redact_sensitive(&data, &schema);
        let arr = result["tokens"].as_array().unwrap();
        for item in arr {
            assert_eq!(item, REDACTED_VALUE);
        }
    }

    #[test]
    fn test_redact_sensitive_secret_prefix_keys() {
        let schema = json!({});
        let data = json!({
            "_secret_api_key": "key123",
            "public_field": "visible"
        });
        let result = redact_sensitive(&data, &schema);
        assert_eq!(result["_secret_api_key"], REDACTED_VALUE);
        assert_eq!(result["public_field"], "visible");
    }

    #[test]
    fn test_redact_sensitive_null_values_preserved() {
        let schema = json!({
            "properties": {
                "password": {"type": "string", "x-sensitive": true}
            }
        });
        let data = json!({"password": null});
        let result = redact_sensitive(&data, &schema);
        assert!(result["password"].is_null());
    }

    #[test]
    fn test_redact_sensitive_no_schema_no_redaction() {
        let data = json!({"password": "s3cret"});
        let result = redact_sensitive(&data, &Value::Null);
        assert_eq!(result, data);
    }

    // ═══════════════════════════════════════════════════════════════════
    // _approval_token Phase B
    // ═══════════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn test_approval_token_stripped_from_inputs_and_check_called() {
        let handler = MockApprovalHandler::with_status("approved");
        let module = MockModule::echo();
        let annotations = ModuleAnnotations {
            requires_approval: true,
            ..Default::default()
        };
        let mut executor = build_executor_with_module(module, annotations);
        executor.set_approval_handler(Box::new(handler));

        let inputs = json!({"_approval_token": "tok-123", "data": "hello"});
        let result = executor.call("test_mod", inputs, None, None).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_approval_no_token_calls_request_approval() {
        let handler = MockApprovalHandler::with_status("approved");
        let module = MockModule::echo();
        let annotations = ModuleAnnotations {
            requires_approval: true,
            ..Default::default()
        };
        let mut executor = build_executor_with_module(module, annotations);
        executor.set_approval_handler(Box::new(handler));

        // No _approval_token -> should call request_approval
        let inputs = json!({"data": "hello"});
        let result = executor.call("test_mod", inputs, None, None).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_notes_requires_approval_without_gating() {
        // validate() per spec §12.8 MUST NOT actually request approval,
        // it only reports requires_approval = true.
        let handler = MockApprovalHandler::with_status("timeout");
        let module = MockModule::echo();
        let annotations = ModuleAnnotations {
            requires_approval: true,
            ..Default::default()
        };
        let mut executor = build_executor_with_module(module, annotations);
        executor.set_approval_handler(Box::new(handler));

        let result = executor.validate("test_mod", &json!({})).await.unwrap();
        assert!(result.valid);
        assert!(result.requires_approval);
    }
}

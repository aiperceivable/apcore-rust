// APCore Protocol — Executor
// Spec reference: Module execution engine

use parking_lot::RwLock;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, LazyLock};

use futures_core::Stream;
use futures_util::StreamExt;
use serde_json::Value;

use crate::acl::ACL;
use crate::approval::ApprovalHandler;
use crate::builtin_steps::{
    build_internal_strategy, build_minimal_strategy, build_performance_strategy,
    build_standard_strategy, build_testing_strategy,
};
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
use crate::registry::registry::{module_id_pattern, Registry};
use crate::utils::propagate_module_error;

/// Maximum nesting depth for deep_merge_value to prevent stack overflow on
/// adversarial or pathological inputs.
///
/// Aligned with apcore-python and apcore-typescript (32) per spec (sync STREAM-001).
const DEEP_MERGE_MAX_DEPTH: usize = 32;

/// Deep-merge a list of JSON Value chunks into a single accumulated Value.
///
/// Retained for tests that explicitly verify the unchecked merge behavior;
/// the streaming pipeline now uses [`deep_merge_chunks_checked`] (D-19) so a
/// non-object chunk surfaces a structured error rather than silently
/// replacing the accumulator.
#[cfg(test)]
fn deep_merge_chunks(chunks: &[Value]) -> Value {
    let mut acc = Value::Null;
    for chunk in chunks {
        deep_merge_value(&mut acc, chunk, 0);
    }
    acc
}

/// Sync finding D-19: deep-merge chunks while *enforcing* that every chunk is
/// a JSON object. A non-object chunk (string, number, array, etc.) yields
/// `ModuleError::GeneralInvalidInput` with `details["code"] =
/// "STREAM_CHUNK_NOT_OBJECT"`, mirroring Python's `_deep_merge` AttributeError
/// and TypeScript's TypeError. The previous Rust path silently replaced the
/// accumulator with a non-object, leaving downstream consumers staring at
/// surprise scalars.
///
/// `chunks` may be empty; in that case the function returns an empty object
/// (mirroring Python's `accumulated: dict[str, Any] = {}` initial state).
pub fn deep_merge_chunks_checked(chunks: &[Value]) -> Result<Value, ModuleError> {
    let mut acc: Value = Value::Object(serde_json::Map::new());
    for (idx, chunk) in chunks.iter().enumerate() {
        if !chunk.is_object() {
            let mut details = std::collections::HashMap::new();
            details.insert(
                "code".to_string(),
                Value::String("STREAM_CHUNK_NOT_OBJECT".to_string()),
            );
            details.insert(
                "chunk_index".to_string(),
                Value::Number(serde_json::Number::from(idx)),
            );
            details.insert(
                "actual_type".to_string(),
                Value::String(json_type_name(chunk).to_string()),
            );
            return Err(ModuleError::new(
                ErrorCode::GeneralInvalidInput,
                format!(
                    "Streaming chunk at index {idx} is not a JSON object \
                     (got {}); chunks must be objects so deep_merge can \
                     accumulate them.",
                    json_type_name(chunk)
                ),
            )
            .with_details(details));
        }
        deep_merge_value(&mut acc, chunk, 0);
    }
    Ok(acc)
}

fn json_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn deep_merge_value(base: &mut Value, overlay: &Value, depth: usize) {
    if depth >= DEEP_MERGE_MAX_DEPTH {
        // At the depth limit, replace rather than recurse to avoid stack overflow.
        *base = overlay.clone();
        return;
    }
    match (base, overlay) {
        (Value::Object(base_map), Value::Object(overlay_map)) => {
            for (k, v) in overlay_map {
                let entry = base_map.entry(k.clone()).or_insert(Value::Null);
                deep_merge_value(entry, v, depth + 1);
            }
        }
        (base, overlay) => {
            *base = overlay.clone();
        }
    }
}

/// Resolve a preset strategy by name.
///
/// Built-in names: `"standard"`, `"internal"`, `"testing"`, `"performance"`, `"minimal"`.
pub fn resolve_strategy_by_name(name: &str) -> Result<ExecutionStrategy, ModuleError> {
    match name {
        "standard" => Ok(build_standard_strategy()),
        "internal" => Ok(build_internal_strategy()),
        "testing" => Ok(build_testing_strategy()),
        "performance" => Ok(build_performance_strategy()),
        "minimal" => Ok(build_minimal_strategy()),
        _ => Err(ModuleError::new(
            ErrorCode::GeneralInvalidInput,
            format!("Unknown strategy name '{name}'. Built-in presets: standard, internal, testing, performance, minimal"),
        )),
    }
}

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
            let error = if passed {
                None
            } else {
                st.result.explanation.as_ref().map(|msg| {
                    serde_json::json!({
                        "code": format!("STEP_{}_FAILED", st.name.to_uppercase()),
                        "message": msg,
                    })
                })
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

/// Internal: result of `Executor::prepare_stream`. Carries everything the
/// streaming body needs to invoke `module.stream()` and run Phase 3.
struct StreamSetup {
    module: Arc<dyn crate::module::Module>,
    inputs: Value,
    context: Context<Value>,
    output_schema: Value,
    middleware_manager: Option<Arc<MiddlewareManager>>,
}

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
        .map(|e| {
            // INVARIANT: HashMap<String, String> always serializes to a JSON object; to_value is infallible.
            serde_json::to_value(e).expect("HashMap<String, String> serialization is infallible")
        })
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
    let Some(properties) = schema.get("properties").and_then(|p| p.as_object()) else {
        return;
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
///
/// Cross-language parity: apcore-python and apcore-typescript accept
/// `(name, strategy)` to the equivalent `Executor.register_strategy()` class
/// method. Use [`register_strategy_from_executor`] for that form
/// (sync finding A-015).
pub fn register_strategy(info: StrategyInfo) {
    let mut registry = STRATEGY_REGISTRY.write();
    // Replace existing entry with same name, or append.
    if let Some(existing) = registry.iter_mut().find(|s| s.name == info.name) {
        *existing = info;
    } else {
        registry.push(info);
    }
}

/// Register a strategy by (name, strategy) pair — cross-language parity form.
///
/// Builds a `StrategyInfo` from the given `ExecutionStrategy` and registers
/// it. Mirrors apcore-python `Executor.register_strategy(name, strategy)`
/// and apcore-typescript `Executor.registerStrategy(name, strategy)`
/// (sync finding A-015).
pub fn register_strategy_by_name(name: impl Into<String>, strategy: &ExecutionStrategy) {
    let name = name.into();
    let mut info = strategy.info();
    info.name = name;
    register_strategy(info);
}

/// List all registered strategy summaries.
pub fn list_strategies() -> Vec<StrategyInfo> {
    STRATEGY_REGISTRY.read().clone()
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
    /// Create a new executor with the given (shared) registry and config.
    ///
    /// Builds a standard execution strategy — all calls go through PipelineEngine.
    /// Accepts either an owned `Registry`/`Config` (convenient for tests) or a
    /// pre-shared `Arc<Registry>`/`Arc<Config>` (required for runtime wiring).
    pub fn new(registry: impl Into<Arc<Registry>>, config: impl Into<Arc<Config>>) -> Self {
        Self {
            registry: registry.into(),
            config: config.into(),
            acl: None,
            approval_handler: None,
            middleware_manager: Arc::new(MiddlewareManager::new()),
            strategy: build_standard_strategy(),
        }
    }

    /// Create a new executor with a strategy resolved by name.
    ///
    /// Built-in preset names: `"standard"`, `"internal"`, `"testing"`,
    /// `"performance"`, `"minimal"`.
    pub fn with_strategy_name(
        registry: impl Into<Arc<Registry>>,
        config: impl Into<Arc<Config>>,
        name: &str,
    ) -> Result<Self, ModuleError> {
        let strategy = resolve_strategy_by_name(name)?;
        Ok(Self {
            registry: registry.into(),
            config: config.into(),
            acl: None,
            approval_handler: None,
            middleware_manager: Arc::new(MiddlewareManager::new()),
            strategy,
        })
    }

    /// Create a new executor with a custom execution strategy.
    pub fn with_strategy(
        registry: impl Into<Arc<Registry>>,
        config: impl Into<Arc<Config>>,
        strategy: ExecutionStrategy,
    ) -> Self {
        Self {
            registry: registry.into(),
            config: config.into(),
            acl: None,
            approval_handler: None,
            middleware_manager: Arc::new(MiddlewareManager::new()),
            strategy,
        }
    }

    /// Create a new executor with all optional parameters.
    pub fn with_options(
        registry: impl Into<Arc<Registry>>,
        config: impl Into<Arc<Config>>,
        middlewares: Option<Vec<Box<dyn Middleware>>>,
        acl: Option<ACL>,
        approval_handler: Option<Box<dyn ApprovalHandler>>,
    ) -> Self {
        let middleware_manager = MiddlewareManager::new();
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
            registry: registry.into(),
            config: config.into(),
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
    ///
    /// Takes `&self` — `MiddlewareManager` uses interior mutability, so the
    /// executor can be held behind a shared reference and still have
    /// middleware added after construction. This removes the previous
    /// `Arc::get_mut` hack that panicked once the middleware manager was
    /// cloned into a pipeline context.
    pub fn use_middleware(&self, middleware: Box<dyn Middleware>) -> Result<(), ModuleError> {
        self.middleware_manager.add(middleware)
    }

    /// Remove a middleware by name.
    pub fn remove(&self, name: &str) -> bool {
        self.middleware_manager.remove(name)
    }

    /// Remove a middleware by name (legacy alias).
    pub fn remove_middleware(&self, name: &str) -> bool {
        self.remove(name)
    }

    /// Validate module_id format before pipeline setup.
    ///
    /// Mirrors `apcore-python.Executor._validate_module_id` and
    /// `apcore-typescript.Executor.validateModuleId`. Fails fast with
    /// `InvalidInput` rather than letting the pipeline's module-lookup step
    /// produce a generic "not found" error for a malformed ID.
    fn validate_module_id(module_id: &str) -> Result<(), ModuleError> {
        if module_id.is_empty() || !module_id_pattern().is_match(module_id) {
            return Err(ModuleError::new(
                ErrorCode::GeneralInvalidInput,
                format!(
                    "Invalid module ID: '{}'. Must match pattern: {}",
                    module_id,
                    crate::registry::registry::MODULE_ID_PATTERN,
                ),
            ));
        }
        Ok(())
    }

    /// Execute (call) a module by ID with the given inputs and context.
    ///
    /// Delegates to `PipelineEngine::run()` using the configured strategy.
    pub async fn call(
        &self,
        module_id: &str,
        inputs: serde_json::Value,
        ctx: Option<&Context<serde_json::Value>>,
        version_hint: Option<&str>,
    ) -> Result<serde_json::Value, ModuleError> {
        Self::validate_module_id(module_id)?;
        let context = match ctx {
            Some(c) => c.clone(),
            None => Context::<serde_json::Value>::new(Identity::new(
                "@external".to_string(),
                "external".to_string(),
                vec![],
                HashMap::new(),
            )),
        };
        let mut pipe_ctx = PipelineContext::new(module_id, inputs, context, self.strategy.name());
        if let Some(hint) = version_hint {
            pipe_ctx.version_hint = Some(hint.to_string());
        }
        self.inject_resources(&mut pipe_ctx);

        // Loop iterates only when a RetrySignal is returned from middleware
        // on_error_outcome; every other path returns or errors on the first
        // attempt (sync finding A-D-017).
        loop {
            match PipelineEngine::run(&self.strategy, &mut pipe_ctx).await {
                Ok((output, trace)) => {
                    if !trace.success {
                        let (aborted_step, explanation) = trace
                            .steps
                            .iter()
                            .find_map(|s| {
                                if s.result.action == "abort" {
                                    Some((s.name.as_str(), s.result.explanation.as_deref()))
                                } else {
                                    None
                                }
                            })
                            .unwrap_or(("unknown", None));
                        return Err(ModuleError::pipeline_abort(aborted_step, explanation));
                    }
                    return Ok(output.unwrap_or(serde_json::Value::Null));
                }
                Err(e) => {
                    // §1.1 fail-fast: PipelineEngine wraps step failures in a
                    // `PipelineStepError`. Unwrap to the original typed error so
                    // public callers (and middleware on_error handlers) see the
                    // underlying cause — matches Python `Executor.call`.
                    let underlying = e.unwrap_pipeline_step_error().unwrap_or(e);
                    // §1.2 fail-fast: MiddlewareManager.execute_before wraps a
                    // failing middleware's error as MiddlewareChainError preserving
                    // the original in details["inner_error"]. Unwrap so callers
                    // can dispatch on the original error class (e.g.
                    // ApprovalDeniedError) — matches apcore-python and
                    // apcore-typescript MiddlewareChainError.original semantics
                    // (sync finding A-D-015).
                    let underlying = underlying
                        .unwrap_middleware_chain_error()
                        .unwrap_or(underlying);
                    // Algorithm A11: decorate with trace_id + module_id before
                    // middleware.on_error and final re-raise — matches
                    // apcore-python executor.py:781-782 and apcore-typescript
                    // executor.ts:352 (sync finding A-D-016).
                    let underlying =
                        propagate_module_error(underlying, module_id, &pipe_ctx.context);
                    // Run middleware on_error hooks in reverse so any registered
                    // recovery / retry middleware can intercept the error.
                    let executed = pipe_ctx.executed_middlewares.clone();
                    if !executed.is_empty() {
                        let outcome = self
                            .middleware_manager
                            .execute_on_error_outcome(
                                module_id,
                                pipe_ctx.inputs.clone(),
                                &underlying,
                                &pipe_ctx.context,
                                &executed,
                            )
                            .await;
                        match outcome {
                            Some(crate::middleware::base::OnErrorOutcome::Recovery(value)) => {
                                return Ok(value);
                            }
                            Some(crate::middleware::base::OnErrorOutcome::Retry(signal)) => {
                                // Reset per-run pipeline state so the next
                                // iteration starts clean (matches Python's
                                // _reset_pipe_ctx_for_retry — sync A-D-017).
                                pipe_ctx.inputs = signal.inputs;
                                pipe_ctx.validated_inputs = None;
                                pipe_ctx.module = None;
                                pipe_ctx.output = None;
                                pipe_ctx.validated_output = None;
                                pipe_ctx.executed_middlewares.clear();
                                continue;
                            }
                            None => {}
                        }
                    }
                    return Err(underlying);
                }
            }
        }
    }

    /// Validate module inputs without executing (steps 1-7, spec §12.3).
    ///
    /// Runs the pipeline in `dry_run` mode — pure steps only, side-effecting
    /// steps are skipped automatically.
    ///
    /// `ctx` is the optional execution context. When provided, call-chain
    /// checks (depth limit, circular-call detection) and ACL caller-identity
    /// matching can run against real caller state. When omitted, an anonymous
    /// `@external` context is synthesized for backward compatibility, in which
    /// case call-chain checks are no-ops.
    ///
    /// Aligned with `apcore-python.Executor.validate(module_id, inputs, context=None)`
    /// and `apcore-typescript.Executor.validate(moduleId, inputs?, context?)` per
    /// PROTOCOL_SPEC §12.2.
    pub async fn validate(
        &self,
        module_id: &str,
        inputs: &serde_json::Value,
        ctx: Option<&Context<serde_json::Value>>,
    ) -> Result<PreflightResult, ModuleError> {
        // Sync finding A-D-010: validate() is a NON-THROWING preflight —
        // a malformed module_id MUST be reported as a failed check, not
        // bubble up as a `ModuleError`. apcore-python and apcore-typescript
        // return a structured PreflightResult with `valid=false` for the
        // module_id check; previously Rust used `?` on validate_module_id
        // which broke that contract for callers expecting validate() to
        // never throw on input-shape errors.
        if let Err(err) = Self::validate_module_id(module_id) {
            let check = PreflightCheckResult {
                check: "module_id".to_string(),
                passed: false,
                error: Some(err.to_dict()),
                warnings: vec![],
            };
            return Ok(PreflightResult {
                valid: false,
                checks: vec![check],
                requires_approval: false,
            });
        }
        let context = ctx.cloned().unwrap_or_else(|| {
            Context::<serde_json::Value>::new(Identity::new(
                "@external".to_string(),
                "external".to_string(),
                vec![],
                HashMap::new(),
            ))
        });
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
                // §1.1 fail-fast: unwrap `PipelineStepError` to the original
                // typed cause for preflight categorization (mirrors Python).
                let underlying = e.unwrap_pipeline_step_error().unwrap_or(e);
                let check_name = match underlying.code {
                    ErrorCode::ModuleNotFound => "module_lookup",
                    ErrorCode::ACLDenied => "acl",
                    ErrorCode::SchemaValidationError | ErrorCode::GeneralInvalidInput => "schema",
                    ErrorCode::CallDepthExceeded | ErrorCode::CircularCall => "call_chain",
                    _ => "unknown",
                };
                checks.push(PreflightCheckResult {
                    check: check_name.to_string(),
                    passed: false,
                    error: Some(serde_json::json!({
                        "code": format!("{:?}", underlying.code),
                        "message": underlying.message,
                    })),
                    warnings: vec![],
                });
            }
        }

        // Detect requires_approval from module annotations.
        let mut requires_approval = false;
        if let Some(desc) = self.registry.get_definition(module_id) {
            if desc
                .annotations
                .as_ref()
                .is_some_and(|a| a.requires_approval)
            {
                requires_approval = true;
            }
        }

        // Invoke module-level preflight — matches apcore-python executor.py:547-571
        // and apcore-typescript executor.ts:632-653 (sync finding A-D-013).
        // Only called when no pipeline step failed (matching Python/TS guard on
        // pipe_ctx.module existing, which implies lookup succeeded).
        let pipeline_ok = checks.iter().all(|c| c.passed);
        if pipeline_ok {
            if let Ok(Some(module)) = self.registry.get(module_id) {
                let preflight_result = module.preflight();
                let passed = preflight_result.valid;
                let error = if passed {
                    None
                } else {
                    preflight_result
                        .checks
                        .iter()
                        .find(|c| !c.passed)
                        .and_then(|c| c.error.clone())
                };
                let warnings: Vec<String> = preflight_result
                    .checks
                    .iter()
                    .flat_map(|c| c.warnings.clone())
                    .collect();
                checks.push(PreflightCheckResult {
                    check: "module_preflight".to_string(),
                    passed,
                    error,
                    warnings,
                });
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
    pub fn from_registry(
        registry: impl Into<Arc<Registry>>,
        config: impl Into<Arc<Config>>,
    ) -> Self {
        Self::new(registry, config)
    }

    /// Stream execution of a module.
    ///
    /// Returns an async `Stream` of output chunks. Each chunk is delivered to
    /// the caller *as soon as it is produced* by the underlying module — no
    /// buffering — so this is true incremental streaming.
    ///
    /// Pipeline phases:
    /// - **Phase 1 (pre-stream):** context creation, call-chain guard, module
    ///   lookup, ACL check, approval gate, before-middleware, input validation.
    ///   Any failure surfaces as the first (and only) `Err` item in the stream.
    /// - **Phase 2 (body):** call `module.stream()`, forward each chunk to the
    ///   caller as it arrives, and accumulate copies into a buffer for Phase 3.
    /// - **Phase 3 (post-stream):** after the inner stream is exhausted,
    ///   deep-merge the accumulated chunks, validate the merged result against
    ///   the module's output schema, then run after-middleware. If either step
    ///   fails, the error is yielded as the final item of the output stream.
    ///
    /// If the module does not implement `stream()` (returns `None`), an error
    /// with `ErrorCode::GeneralNotImplemented` is yielded.
    pub fn stream<'a>(
        &'a self,
        module_id: &str,
        inputs: Value,
        ctx: Option<&Context<Value>>,
        version_hint: Option<&str>,
    ) -> Pin<Box<dyn Stream<Item = Result<Value, ModuleError>> + Send + 'a>> {
        // Capture by value so the returned Stream is `'a` (only borrowing &self).
        let module_id_owned = module_id.to_string();
        let version_hint_owned = version_hint.map(str::to_string);
        let initial_context = ctx.cloned();

        Box::pin(async_stream::try_stream! {
            // Fail fast for invalid module IDs before setting up any pipeline state.
            Self::validate_module_id(&module_id_owned)?;

            // Phase 1: pre-stream setup. Any error short-circuits the whole stream.
            let mut setup = self
                .prepare_stream(
                    &module_id_owned,
                    inputs,
                    initial_context,
                    version_hint_owned.as_deref(),
                )
                .await?;

            // Phase 2: invoke module.stream() and forward chunks as they arrive.
            // Note: individual chunks are NOT validated against output_schema here;
            // validation runs on the deep-merged result in Phase 3 after the stream
            // is exhausted. See Module::stream contract for details.
            //
            // Fallback (sync STREAM-002): if Module::stream returns None, fall back
            // to Module::execute() and yield the result as a single chunk.
            // Mirrors apcore-python/src/apcore/executor.py:862-865 and
            // apcore-typescript/src/executor.ts:519-522.
            let stream_handle = setup.module.stream(setup.inputs.clone(), &setup.context);
            let mut accumulated: Vec<Value> = Vec::new();

            if let Some(mut inner) = stream_handle {
                while let Some(chunk_result) = inner.next().await {
                    // STREAM-003 (sync): enforce global_deadline between chunks.
                    if let Some(deadline) = setup.context.global_deadline {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs_f64();
                        if now >= deadline {
                            Err(ModuleError::new(
                                ErrorCode::ModuleTimeout,
                                format!(
                                    "Module '{module_id_owned}' streaming aborted: global deadline exceeded"
                                ),
                            ))?;
                            return;
                        }
                    }
                    let chunk = chunk_result?;
                    accumulated.push(chunk.clone());
                    yield chunk;
                }
            } else {
                // Fallback: module doesn't support streaming. Run execute() and
                // yield its result as a single chunk.
                let output = setup
                    .module
                    .execute(setup.inputs.clone(), &setup.context)
                    .await?;
                accumulated.push(output.clone());
                yield output;
            }

            // Phase 3: post-stream validation + middleware_after on the merged
            // output. Chunks are already delivered, so failures here are
            // SWALLOWED with a tracing::warn rather than yielded as a final
            // Err item — matches apcore-python (executor.py:920 emits
            // `apcore.stream.post_validation_failed` event and does not raise)
            // and apcore-typescript (console.warn + swallow) behavior
            // (sync finding A-D-012).
            // D-19: enforce that all chunks are objects before merging. A
            // non-object chunk is logged and skips post-stream validation
            // (chunks have already been delivered to the consumer).
            let merged = match deep_merge_chunks_checked(&accumulated) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(
                        module_id = %module_id_owned,
                        error = %e.message,
                        "stream phase-3 chunk shape check failed (chunks already delivered, swallowed)"
                    );
                    let _ = &mut setup;
                    return;
                }
            };
            if let Err(e) = validate_against_schema(&merged, &setup.output_schema, "Output") {
                tracing::warn!(
                    module_id = %module_id_owned,
                    error = %e.message,
                    "stream phase-3 output schema validation failed (chunks already delivered, swallowed)"
                );
            } else if let Some(ref mm) = setup.middleware_manager {
                if let Err(e) = mm
                    .execute_after(
                        &module_id_owned,
                        setup.inputs.clone(),
                        merged,
                        &setup.context,
                    )
                    .await
                {
                    tracing::warn!(
                        module_id = %module_id_owned,
                        error = %e.message,
                        "stream phase-3 middleware_after failed (chunks already delivered, swallowed)"
                    );
                }
            }
            // We intentionally do NOT yield the merged result — chunks are the
            // payload, Phase 3 is pure side effects (validation + observation).
            let _ = &mut setup; // silence unused-mut on the no-error path
        })
    }

    /// Run Phase 1 of the streaming pipeline: every step up to (but not
    /// including) `execute`. Returns the resolved module, the (possibly
    /// middleware-mutated) inputs, the prepared context, the module's output
    /// schema, and a handle to the middleware manager for after-middleware.
    ///
    /// Drives the shared [`PipelineEngine::run_until_step`] so every per-step
    /// declaration (`match_modules`, `ignore_errors`, `timeout_ms`, `dry_run`
    /// purity filtering, `skip_to` targets) behaves identically to the
    /// non-streaming `call()` path. A prior audit found this path had a
    /// bespoke loop that ignored those declarations and silently diverged.
    async fn prepare_stream(
        &self,
        module_id: &str,
        inputs: Value,
        ctx: Option<Context<Value>>,
        version_hint: Option<&str>,
    ) -> Result<StreamSetup, ModuleError> {
        let context = ctx.unwrap_or_else(|| {
            Context::<Value>::new(Identity::new(
                "@external".to_string(),
                "external".to_string(),
                vec![],
                HashMap::new(),
            ))
        });

        let mut pipe_ctx = PipelineContext::new(module_id, inputs, context, self.strategy.name());
        if let Some(hint) = version_hint {
            pipe_ctx.version_hint = Some(hint.to_string());
        }
        self.inject_resources(&mut pipe_ctx);

        // Drive the shared pipeline engine up to — but not including — the
        // `execute` step. This inherits all per-step metadata handling from
        // `PipelineEngine::run`, so streaming and non-streaming never diverge.
        // §1.1 fail-fast: unwrap PipelineStepError to surface the original
        // typed error to the streaming caller.
        let (_output, trace) =
            PipelineEngine::run_until_step(&self.strategy, &mut pipe_ctx, "execute")
                .await
                .map_err(|e| e.unwrap_pipeline_step_error().unwrap_or(e))?;

        // `run_until` returns `Ok` for pipeline-level aborts (so the caller
        // can observe the trace). Streaming requires a resolved module, so
        // an aborted pre-execute phase is an error for us.
        if !trace.success {
            let explanation = trace
                .steps
                .iter()
                .find_map(|s| {
                    if s.result.action == "abort" {
                        s.result.explanation.clone()
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| "pre-stream pipeline aborted".to_string());
            return Err(ModuleError::new(
                ErrorCode::GeneralInternalError,
                explanation,
            ));
        }

        let module = pipe_ctx.module.clone().ok_or_else(|| {
            ModuleError::new(
                ErrorCode::ModuleNotFound,
                format!("Module '{module_id}' was not resolved during pre-stream setup"),
            )
        })?;
        let output_schema = module.output_schema();

        Ok(StreamSetup {
            module,
            inputs: pipe_ctx.inputs,
            context: pipe_ctx.context,
            output_schema,
            middleware_manager: pipe_ctx.middleware_manager.clone(),
        })
    }

    /// Get a reference to the executor's execution strategy.
    pub fn strategy(&self) -> &ExecutionStrategy {
        &self.strategy
    }

    /// Return structured info about the configured pipeline.
    ///
    /// Returns a [`StrategyInfo`] describing the strategy name, step count,
    /// step names, and auto-generated description. This matches the spec and
    /// aligns with the Python and TypeScript SDK return types.
    ///
    /// Use `.to_string()` on the result for a human-readable summary.
    pub fn describe_pipeline(&self) -> StrategyInfo {
        self.strategy.info()
    }

    /// Register a strategy's info in the global registry for introspection.
    ///
    /// Delegates to the module-level [`register_strategy`] function.
    #[deprecated(
        since = "0.20.0",
        note = "Use the module-level `register_strategy` function directly."
    )]
    pub fn register_strategy(info: StrategyInfo) {
        register_strategy(info);
    }

    /// List all registered strategy summaries.
    ///
    /// Delegates to the module-level [`list_strategies`] function.
    #[deprecated(
        since = "0.20.0",
        note = "Use the module-level `list_strategies` function directly."
    )]
    pub fn list_strategies() -> Vec<StrategyInfo> {
        list_strategies()
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
                HashMap::new(),
            )),
        };

        let mut pipeline_ctx =
            PipelineContext::new(module_id, inputs, context, effective_strategy.name());
        self.inject_resources(&mut pipeline_ctx);

        // §1.1 fail-fast: PipelineStepError wraps step failures; unwrap so
        // public callers see the original typed error.
        let (output, trace) = PipelineEngine::run(effective_strategy, &mut pipeline_ctx)
            .await
            .map_err(|e| e.unwrap_pipeline_step_error().unwrap_or(e))?;

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
    pub fn use_before(&self, middleware: Box<dyn BeforeMiddleware>) -> Result<(), ModuleError> {
        self.middleware_manager
            .add(Box::new(BoxedBeforeMiddlewareAdapter(middleware)))
    }

    /// Add an after middleware.
    pub fn use_after(&self, middleware: Box<dyn AfterMiddleware>) -> Result<(), ModuleError> {
        self.middleware_manager
            .add(Box::new(BoxedAfterMiddlewareAdapter(middleware)))
    }
}

// These boxed adapters wrap `Box<dyn BeforeMiddleware>` / `Box<dyn AfterMiddleware>`
// (unsized trait objects) into the full `Middleware` trait. They are private to
// this module because they are only needed by `Executor::use_before` /
// `Executor::use_after`.

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
        fn description(&self) -> &'static str {
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
        let registry = Registry::new();
        let descriptor = ModuleDescriptor {
            module_id: "test_mod".to_string(),
            name: None,
            description: module.description().to_string(),
            documentation: None,
            input_schema: module.input_schema(),
            output_schema: module.output_schema(),
            version: "1.0.0".to_string(),
            tags: vec![],
            annotations: Some(annotations),
            examples: vec![],
            metadata: std::collections::HashMap::new(),
            display: None,
            sunset_date: None,
            dependencies: vec![],
            enabled: true,
        };
        registry
            .register("test_mod", Box::new(module), descriptor)
            .unwrap();
        Executor::new(registry, Config::default())
    }

    // ═══════════════════════════════════════════════════════════════════
    // deep_merge depth cap (sync STREAM-001)
    // ═══════════════════════════════════════════════════════════════════

    /// Build a chain of N nested objects: {"a": {"a": {"a": {... value}}}}.
    fn build_nested(depth: usize, leaf_key: &str, leaf_val: &serde_json::Value) -> Value {
        let mut current = serde_json::json!({ leaf_key: leaf_val });
        for _ in 0..depth {
            current = serde_json::json!({ "a": current });
        }
        current
    }

    #[test]
    fn test_deep_merge_depth_cap_is_32_aligned_with_python_typescript() {
        // Confirm constant value matches spec (sync STREAM-001).
        assert_eq!(DEEP_MERGE_MAX_DEPTH, 32);
    }

    #[test]
    fn test_deep_merge_caps_recursion_at_max_depth() {
        // Two chunks, each 33 levels deep, with conflicting leaves. Once the
        // merge crosses DEEP_MERGE_MAX_DEPTH (32), the second chunk's value
        // replaces the first wholesale instead of recursing further. This
        // guarantees no stack overflow on adversarial inputs.
        let chunk_a = build_nested(33, "x", &serde_json::json!(1));
        let chunk_b = build_nested(33, "y", &serde_json::json!(2));
        // Should not stack-overflow; the deeper merge replaces rather than recurses.
        let merged = deep_merge_chunks(&[chunk_a, chunk_b]);
        // Walking 32 levels of "a" should still yield an object (the cap node).
        let mut cursor = &merged;
        for _ in 0..32 {
            cursor = cursor.get("a").expect("nested 'a' should exist within cap");
        }
        // At/above the cap, the second chunk replaces the first verbatim, so
        // the leaf key from chunk_b dominates.
        let inner_str = cursor.to_string();
        assert!(
            inner_str.contains("\"y\""),
            "at depth >= cap, the later chunk should fully replace the earlier one; got {inner_str}"
        );
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

        let result = executor
            .validate("test_mod", &json!({}), None)
            .await
            .unwrap();
        assert!(result.valid);
        assert!(result.requires_approval);
    }
}

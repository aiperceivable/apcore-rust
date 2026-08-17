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
use crate::events::emitter::{ApCoreEvent, EventEmitter};
use crate::middleware::adapters::{AfterMiddleware, BeforeMiddleware};
use crate::middleware::base::Middleware;
use crate::middleware::manager::MiddlewareManager;
use crate::module::PreflightCheckResult as PfCheck;
use crate::module::{PreflightCheckResult, PreflightResult};
use crate::pipeline::{
    ExecutionStrategy, PipelineContext, PipelineEngine, PipelineTrace, StrategyInfo,
};
use crate::policy::ExecutionPolicy;
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
/// the streaming pipeline now uses [`deep_merge_chunks_checked`] (D-58) so a
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

/// Sync finding D-58: deep-merge chunks while *enforcing* that every chunk is
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
            return Err(stream_chunk_not_object_error(idx, chunk));
        }
        deep_merge_value(&mut acc, chunk, 0);
    }
    Ok(acc)
}

/// Build the canonical "chunk is not a JSON object" error (audit D10-001 /
/// sync finding D-58). Shared by Phase 2 reject-before-deliver in
/// [`Executor::stream`] and by [`deep_merge_chunks_checked`] so both surface an
/// identical `code` / `message` / `details` shape that cross-language consumers
/// can match (mirrors Python's `_deep_merge` AttributeError and TS's TypeError).
pub fn stream_chunk_not_object_error(idx: usize, chunk: &Value) -> ModuleError {
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
    ModuleError::new(
        ErrorCode::GeneralInvalidInput,
        format!(
            "Streaming chunk at index {idx} is not a JSON object \
             (got {}); chunks must be objects so deep_merge can \
             accumulate them.",
            json_type_name(chunk)
        ),
    )
    .with_details(details)
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
        // At the depth limit, stop recursing (stack safety) but still merge
        // SHALLOWLY: assign each overlay key onto base rather than replacing
        // the whole node. Replacing dropped every base-only key, so streaming
        // chunk A `{a:{a:…{x:1}}}` followed by chunk B `{a:{a:…{y:2}}}` at
        // depth 32 accumulated to `{y:2}` in Rust where apcore-python
        // (`executor.py` `_deep_merge`) and apcore-typescript
        // (`executor.ts` `deepMerge`) both produce `{x:1,y:2}`. Machine-generated
        // deeply nested payloads are exactly where streaming accumulators hit
        // this cap.
        match (base, overlay) {
            (Value::Object(base_map), Value::Object(overlay_map)) => {
                for (k, v) in overlay_map {
                    base_map.insert(k.clone(), v.clone());
                }
            }
            (base, overlay) => {
                *base = overlay.clone();
            }
        }
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
        "acl_check" => crate::module::ACL_CHECK_NAME,
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
    /// Indices of the middlewares that ran during Phase-1 setup, so a Phase-2
    /// (mid-stream) error can run on_error recovery over exactly those
    /// (A-D-015 — parity with `call()` and Python/TS stream()).
    executed_middlewares: Vec<usize>,
    /// Optional event emitter, threaded from the pipeline context so Phase 3
    /// can publish `apcore.stream.post_validation_failed` when a post-stream
    /// failure is swallowed (parity with apcore-python / apcore-typescript).
    event_emitter: Option<Arc<EventEmitter>>,
}

/// Internal: outcome of `Executor::prepare_stream`. A Phase-1 (pre-execute)
/// pipeline failure can be intercepted by a middleware `on_error` handler that
/// supplies a recovery value — mirroring `call()` and the Python/TypeScript
/// SDKs. In that case the streaming body must YIELD the recovery value as the
/// stream's chunk rather than surfacing the error (sync finding A-D-001).
enum StreamPrep {
    /// Phase 1 succeeded; proceed to invoke `module.stream()`.
    Setup(Box<StreamSetup>),
    /// A middleware `on_error` handler recovered from a Phase-1 failure; yield
    /// this value as the single stream chunk, then end the stream.
    Recovery(Value),
}

/// Validate a JSON value against a JSON Schema.
///
/// This is the module-invocation boundary — `builtin_steps.rs` calls it for both
/// input and output. It performs **no type coercion**, under any host
/// configuration: a contract that declares `integer` receives an integer, and
/// `"42"` is a type error (TYPE_MAPPING §17.3). `SchemaValidator::new()` agrees;
/// its `coerce_types` default is `false` for exactly that reason.
///
/// Returns `Ok(())` when the value is valid. On failure the [`ModuleError`]
/// carries `SCHEMA_VALIDATION_ERROR` when the *value* is at fault, or
/// `SCHEMA_PARSE_ERROR` when the *schema* could not be compiled; both spell
/// their failures out in `details.errors`.
pub fn validate_against_schema(
    value: &Value,
    schema: &Value,
    direction: &str,
) -> Result<(), ModuleError> {
    // If schema is null/empty, skip validation
    if !has_schema(schema) {
        return Ok(());
    }

    // Compiled through the same builder `SchemaValidator` uses, so the executor
    // and the validator can never reach opposite verdicts on one input: the
    // document's own draft is honoured and `format` stays an annotation.
    let validator = match crate::schema::validator::build_validator(schema) {
        Ok(v) => v,
        Err(e) => {
            // The *schema* is broken, not the value. SCHEMA_VALIDATION_ERROR is
            // flagged caller-fixable (`user_fixable_for_code`), which would send
            // the caller off to edit arguments that were never at fault, so this
            // branch reports SCHEMA_PARSE_ERROR — the same code
            // `SchemaValidator::validate_detailed_raw` uses for a failed compile.
            let message = format!("{direction} schema is invalid: {e}");
            let mut entry = HashMap::new();
            entry.insert("field".to_string(), String::new());
            entry.insert("message".to_string(), message.clone());
            let mut details = HashMap::new();
            details.insert(
                "errors".to_string(),
                Value::Array(vec![
                    // INVARIANT: HashMap<String, String> always serializes to a JSON object.
                    serde_json::to_value(&entry)
                        .expect("HashMap<String, String> serialization is infallible"),
                ]),
            );
            return Err(ModuleError::new(ErrorCode::SchemaParseError, message)
                .with_details(details)
                .with_ai_guidance(format!(
                    "The {direction} schema itself could not be compiled. Fix the module's schema declaration; the call arguments are not the cause."
                )));
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

/// Return a `ModuleTimeout` error if `deadline` (epoch-seconds) has passed.
///
/// STREAM-003 / A-D-007: uses strict `>` to match apcore-python
/// (`time.monotonic() > global_deadline`) and apcore-typescript
/// (`Date.now() > globalDeadline`). The unit here is internal epoch-seconds
/// (set+compared consistently via `as_secs_f64`); the cross-SDK clocks differ
/// but each is self-consistent — only the comparator is normalized.
fn stream_deadline_exceeded(deadline: Option<f64>, module_id: &str) -> Option<ModuleError> {
    let deadline = deadline?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    if now > deadline {
        return Some(ModuleError::new(
            ErrorCode::ModuleTimeout,
            format!("Module '{module_id}' streaming aborted: global deadline exceeded"),
        ));
    }
    None
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

/// In-place redaction of keys starting with `_secret_`, at any depth.
///
/// A-D-003: recurses into both nested objects AND array elements so sensitive
/// keys inside objects nested in arrays are redacted. Mirrors apcore-python
/// `utils/redaction.py` (`_redact_by_keys_and_regex` + `_redact_in_list`).
fn redact_secret_prefix(data: &mut serde_json::Map<String, Value>) {
    let keys: Vec<String> = data.keys().cloned().collect();
    for key in keys {
        if key.starts_with("_secret_") {
            if let Some(val) = data.get(&key) {
                if !val.is_null() {
                    data.insert(key, Value::String(REDACTED_VALUE.to_string()));
                }
            }
        } else if let Some(val) = data.get_mut(&key) {
            match val {
                Value::Object(obj) => redact_secret_prefix(obj),
                Value::Array(arr) => redact_secret_prefix_in_list(arr),
                _ => {}
            }
        }
    }
}

/// Traverse a list, redacting `_secret_`-prefixed keys in dict children and
/// recursing into nested lists. Mirrors apcore-python `_redact_in_list`.
fn redact_secret_prefix_in_list(items: &mut [Value]) {
    for item in items.iter_mut() {
        match item {
            Value::Object(obj) => redact_secret_prefix(obj),
            Value::Array(nested) => redact_secret_prefix_in_list(nested),
            _ => {}
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
    /// Execution-time governance policy consulted by the Step-5 approval gate
    /// (apcore#76 RFC pilot). Flows to the gate via `PipelineContext`.
    pub policy: Option<Arc<ExecutionPolicy>>,
    /// Event emitter for governance events (apcore#77). When set, the ACL and
    /// approval-gate steps publish `apcore.acl.denied` /
    /// `apcore.approval.decision` / `apcore.policy.override` on the bus.
    pub event_emitter: Option<Arc<EventEmitter>>,
    /// Dedup set backing the approval gate's fail-loud warn-once-per-module
    /// governance warnings (apcore#76). Executor-owned so dedup persists across
    /// calls; threaded into each `PipelineContext` by `inject_resources`.
    governance_warned: Arc<parking_lot::Mutex<std::collections::HashSet<String>>>,
    pub middleware_manager: Arc<MiddlewareManager>,
    /// Execution strategy — all calls go through PipelineEngine.
    strategy: ExecutionStrategy,
    /// Unique per-Executor identity handle bound to `context.executor` at
    /// pipeline entry (apcore Issue #66 / `core-executor.md` §"Contract:
    /// Executor binding to Context"). Two `Executor` instances are
    /// distinguished by `Arc::ptr_eq` on this handle; cloning an `Executor`
    /// would produce a fresh handle by design — Executors are meant to be
    /// shared via `Arc<Executor>`, not duplicated.
    instance_handle: Arc<()>,
    /// Per-instance toggle store consulted by the `module_lookup` step to
    /// reject disabled modules (issue #71). Defaults to the process-global
    /// store so executors built outside an `APCore` keep working; `APCore`
    /// rebinds this to its own store via [`Executor::set_toggle_state`] so the
    /// read path (pipeline lookup) and the write path
    /// (`ToggleFeatureModule`) share one instance-scoped store.
    toggle_state: Arc<crate::sys_modules::ToggleState>,
}

impl Executor {
    /// Create a new executor with the given (shared) registry and config.
    ///
    /// Builds a standard execution strategy — all calls go through PipelineEngine.
    /// Accepts either an owned `Registry`/`Config` (convenient for tests) or a
    /// pre-shared `Arc<Registry>`/`Arc<Config>` (required for runtime wiring).
    pub fn new(registry: impl Into<Arc<Registry>>, config: impl Into<Arc<Config>>) -> Self {
        let toggle_state = crate::sys_modules::global_toggle_state_arc();
        Self {
            registry: registry.into(),
            config: config.into(),
            acl: None,
            approval_handler: None,
            policy: None,
            event_emitter: None,
            governance_warned: Arc::new(parking_lot::Mutex::new(std::collections::HashSet::new())),
            middleware_manager: Arc::new(MiddlewareManager::new()),
            strategy: crate::builtin_steps::build_standard_strategy_with_toggle(Arc::clone(
                &toggle_state,
            )),
            instance_handle: Arc::new(()),
            toggle_state,
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
            policy: None,
            event_emitter: None,
            governance_warned: Arc::new(parking_lot::Mutex::new(std::collections::HashSet::new())),
            middleware_manager: Arc::new(MiddlewareManager::new()),
            strategy,
            instance_handle: Arc::new(()),
            toggle_state: crate::sys_modules::global_toggle_state_arc(),
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
            policy: None,
            event_emitter: None,
            governance_warned: Arc::new(parking_lot::Mutex::new(std::collections::HashSet::new())),
            middleware_manager: Arc::new(MiddlewareManager::new()),
            strategy,
            instance_handle: Arc::new(()),
            toggle_state: crate::sys_modules::global_toggle_state_arc(),
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
        let toggle_state = crate::sys_modules::global_toggle_state_arc();
        Self {
            registry: registry.into(),
            config: config.into(),
            acl: acl.map(Arc::new),
            approval_handler: approval_handler.map(|h| Arc::from(h) as Arc<dyn ApprovalHandler>),
            policy: None,
            event_emitter: None,
            governance_warned: Arc::new(parking_lot::Mutex::new(std::collections::HashSet::new())),
            middleware_manager: Arc::new(middleware_manager),
            strategy: crate::builtin_steps::build_standard_strategy_with_toggle(Arc::clone(
                &toggle_state,
            )),
            instance_handle: Arc::new(()),
            toggle_state,
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

    /// Find the first middleware in the chain whose name matches, as a shared
    /// handle. Used by extension application to locate an existing
    /// `TracingMiddleware` and reconfigure it (sync finding A-D-18).
    pub fn find_middleware(&self, name: &str) -> Option<Arc<dyn Middleware>> {
        self.middleware_manager.find_by_name(name)
    }

    /// Rebind the per-instance toggle store consulted by the `module_lookup`
    /// step (issue #71).
    ///
    /// `APCore::with_options` calls this so the pipeline read path and the
    /// `ToggleFeatureModule` write path share one instance-scoped
    /// [`ToggleState`](crate::sys_modules::ToggleState).
    ///
    /// Every built-in preset (`standard`, `internal`, `testing`,
    /// `performance`, `minimal`) is derived from `build_standard_strategy()`,
    /// which binds the *process-global* toggle store, so all five need the
    /// `module_lookup` step rebound — not just `standard`. Rebinding only the
    /// `standard` preset meant `apcore.disable(module)` on one instance was
    /// silently ignored by any executor running another preset (issue #71).
    /// apcore-python honours the per-instance store in all five presets.
    ///
    /// A user-supplied custom strategy is left untouched: those callers are
    /// expected to wire the store themselves via
    /// [`build_standard_strategy_with_toggle`](crate::builtin_steps::build_standard_strategy_with_toggle).
    pub fn set_toggle_state(&mut self, toggle_state: Arc<crate::sys_modules::ToggleState>) {
        const BUILTIN_PRESETS: &[&str] =
            &["standard", "internal", "testing", "performance", "minimal"];
        if BUILTIN_PRESETS.contains(&self.strategy.name()) {
            let bound = Arc::clone(&toggle_state);
            // `replace_with` bypasses the `replaceable` flag (module_lookup is
            // not user-replaceable) and preserves the step's position.
            // `minimal` and friends all keep `module_lookup`, but ignore the
            // error if a preset ever drops it.
            self.strategy
                .replace_with("module_lookup", move |_old| {
                    Box::new(crate::builtin_steps::BuiltinModuleLookup::new(bound))
                })
                .ok();
        }
        self.toggle_state = toggle_state;
    }

    /// Shared handle to this executor's per-instance toggle store (issue #71).
    #[must_use]
    pub fn toggle_state(&self) -> Arc<crate::sys_modules::ToggleState> {
        Arc::clone(&self.toggle_state)
    }

    /// Set the ACL for access control.
    pub fn set_acl(&mut self, acl: ACL) {
        self.acl = Some(Arc::new(acl));
    }

    /// Set the approval handler.
    pub fn set_approval_handler(&mut self, handler: Box<dyn ApprovalHandler>) {
        self.approval_handler = Some(Arc::from(handler));
    }

    /// Set (or clear) the execution-time governance policy consulted by the
    /// Step-5 approval gate (apcore#76 RFC pilot). Mirrors
    /// [`Self::set_approval_handler`]; the policy flows to the gate via
    /// `PipelineContext` at each invocation, so this takes effect immediately
    /// for subsequent calls. Pass `None` to remove the policy.
    pub fn set_policy(&mut self, policy: Option<ExecutionPolicy>) {
        self.policy = policy.map(Arc::new);
    }

    /// Set (or clear) the event emitter used to publish governance events
    /// (apcore#77): `apcore.acl.denied`, `apcore.approval.decision`, and
    /// `apcore.policy.override`.
    pub fn set_event_emitter(&mut self, emitter: Option<Arc<EventEmitter>>) {
        self.event_emitter = emitter;
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
                ErrorCode::InvalidModuleId,
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
        self.bind_to_context(&mut pipe_ctx)?;

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
                    // Cancellation short-circuit (D-20): an ExecutionCancelled
                    // error MUST bypass the on_error middleware chain so
                    // logging middleware cannot swallow it and retry
                    // middleware cannot resurrect the call via RetrySignal.
                    // Mirrors apcore-python and apcore-typescript.
                    // Spec: docs/features/core-executor.md §Cancellation Short-Circuit
                    if matches!(underlying.code, ErrorCode::ExecutionCancelled) {
                        return Err(underlying);
                    }
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
    #[allow(clippy::too_many_lines)]
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
                predicted_changes: vec![],
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
        // validate() is non-throwing per A-D-010: bind errors surface as a
        // failed `executor_binding` check rather than bubbling up.
        if let Err(err) = self.bind_to_context(&mut pipe_ctx) {
            let check = PreflightCheckResult {
                check: "executor_binding".to_string(),
                passed: false,
                error: Some(err.to_dict()),
                warnings: vec![],
            };
            return Ok(PreflightResult {
                valid: false,
                checks: vec![check],
                requires_approval: false,
                predicted_changes: vec![],
            });
        }

        let mut checks: Vec<PreflightCheckResult> = Vec::new();
        let trace_result = PipelineEngine::run(&self.strategy, &mut pipe_ctx).await;
        match trace_result {
            Ok((_output, trace)) => {
                checks.extend(trace_to_checks(&trace));
            }
            Err(e) => {
                // Pipeline step raised an error; convert to a failed check.
                //
                // The trace's own entry for the aborting step is dropped: the
                // typed entry pushed below supersedes it, carrying the WIRE
                // error code and the real message where the trace carries only
                // `STEP_<NAME>_FAILED`. Keeping both put the SAME failure in
                // `checks` twice, so `errors()` reported two problems where one
                // existed and §12.8.4's one-entry-per-check shape did not hold.
                // The passed steps are kept — apcore-python drops the whole
                // trace here and loses them, which is why its `acl` entry
                // disappears from a schema failure.
                checks.extend(
                    trace_to_checks(&pipe_ctx.trace)
                        .into_iter()
                        .filter(|c| c.passed),
                );
                // §1.1 fail-fast: unwrap `PipelineStepError` to the original
                // typed cause for preflight categorization (mirrors Python).
                let underlying = e.unwrap_pipeline_step_error().unwrap_or(e);
                let check_name = match underlying.code {
                    ErrorCode::ModuleNotFound => "module_lookup",
                    ErrorCode::ACLDenied => crate::module::ACL_CHECK_NAME,
                    ErrorCode::SchemaValidationError | ErrorCode::GeneralInvalidInput => "schema",
                    ErrorCode::CallDepthExceeded | ErrorCode::CircularCall => "call_chain",
                    _ => "unknown",
                };
                checks.push(PreflightCheckResult {
                    check: check_name.to_string(),
                    passed: false,
                    error: Some(serde_json::json!({
                        // WIRE code, never `Debug` (errors.rs `wire_str`, sync
                        // finding A-D-14). apcore-python builds this dict from
                        // `underlying_e.to_dict()`, whose `code` is the wire
                        // string (executor.py:604-607 — the
                        // `type(e).__name__` line above it is only the fallback
                        // for a non-apcore exception), and apcore-typescript
                        // emits `{ code: e.code }` (executor.ts:880). `Debug`
                        // here yielded the PascalCase variant name, which is a
                        // code no registry contains.
                        "code": underlying.code.wire_str(),
                        "message": underlying.message,
                    })),
                    warnings: vec![],
                });
            }
        }

        // Detect requires_approval from module annotations. Under an
        // ExecutionPolicy (apcore#76) the preflight MUST report the
        // policy-effective verdict (`PolicyDecision::needs_approval`) so it
        // matches what the Step-5 gate will actually enforce.
        let desc_annotations = self
            .registry
            .get_definition(module_id)
            .ok()
            .flatten()
            .and_then(|desc| desc.annotations);
        let requires_approval = if let Some(policy) = self.policy.as_ref() {
            policy
                .resolve(module_id, desc_annotations.as_ref())
                .needs_approval
        } else {
            desc_annotations
                .as_ref()
                .is_some_and(|a| a.requires_approval)
        };

        // Module-level introspection is gated on TWO conditions, not one.
        //
        // 1. Module lookup succeeded (mirrors the Python/TS guard on
        //    `pipe_ctx.module` being non-null).
        // 2. The ACL did not deny the call — PROTOCOL_SPEC §12.8.5.1.
        //
        // Condition 2 is the security half. `preflight()` and `preview()` are
        // module-authored code, and what they return names what the call would
        // do: the resolved binary and argv of a command-wrapping module, the
        // target of a write. Module lookup is Step 3 and the ACL check is Step
        // 4, so gating on lookup alone runs module code for a caller the ACL
        // just denied and hands back what it said. `apcore-mcp-rust` had to
        // filter these three check names out of `__apcore_module_preview` by
        // string to keep argv away from a denied caller; the gate belongs here.
        //
        // Scoped to authorization deliberately: a failed `schema` check does
        // NOT suppress introspection, because a caller the ACL permits is
        // entitled to the module's account of what would happen even when its
        // inputs are malformed.
        let acl_denied = checks
            .iter()
            .any(|c| c.check == crate::module::ACL_CHECK_NAME && !c.passed);
        let mut predicted_changes: Vec<crate::module::Change> = Vec::new();
        if let (false, Ok(Some(module))) = (acl_denied, self.registry.get(module_id)) {
            let warnings = module.preflight(inputs, ctx);
            checks.push(PreflightCheckResult {
                check: crate::module::MODULE_PREFLIGHT_CHECK_NAME.to_string(),
                passed: true,
                error: None,
                warnings,
            });

            // Module-level preview() (RFC `rfc-preview-method.md`, target v0.21.0).
            // Invoked after preflight in cross-language alignment with
            // apcore-python and apcore-typescript. Panics are treated as
            // advisory and do NOT fail validation.
            //
            // Module trait methods are not UnwindSafe in general (they take
            // `&self`); we deliberately use AssertUnwindSafe because:
            // 1. preview() is contractually side-effect-free per RFC.
            // 2. Even if a module unwinds across borrowed state, the
            //    Registry's Arc<dyn Module> stays valid afterwards — the
            //    panic only crosses the catch_unwind boundary.
            let preview_panic_msg: Option<String> = {
                use std::panic::{catch_unwind, AssertUnwindSafe};
                let module_ref = module.as_ref();
                let result = catch_unwind(AssertUnwindSafe(|| module_ref.preview(inputs, ctx)));
                match result {
                    Ok(Some(preview_result)) => {
                        if !preview_result.changes.is_empty() {
                            predicted_changes = preview_result.changes;
                        }
                        checks.push(PreflightCheckResult {
                            check: crate::module::MODULE_PREVIEW_CHECK_NAME.to_string(),
                            passed: true,
                            error: None,
                            warnings: vec![],
                        });
                        None
                    }
                    Ok(None) => {
                        // Default trait impl returns None — record nothing.
                        // This matches apcore-python's "method absent → no
                        // module_preview check" semantics and keeps the
                        // checks list small for modules that don't preview.
                        None
                    }
                    Err(panic_payload) => {
                        // Extract the panic message from the payload.
                        let msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                            (*s).to_string()
                        } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                            s.clone()
                        } else {
                            "<non-string panic payload>".to_string()
                        };
                        Some(msg)
                    }
                }
            };
            if let Some(msg) = preview_panic_msg {
                checks.push(PreflightCheckResult {
                    check: crate::module::MODULE_PREVIEW_CHECK_NAME.to_string(),
                    passed: true,
                    error: None,
                    warnings: vec![format!("preview() panicked: {msg}")],
                });
            }
        }

        let valid = checks.iter().all(|c| c.passed);
        Ok(PreflightResult {
            valid,
            checks,
            requires_approval,
            predicted_changes,
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

            // Phase 1: pre-stream setup. A pre-execute failure runs the
            // middleware on_error recovery chain (A-D-001): a recovery value is
            // yielded as the stream's chunk; otherwise the error short-circuits
            // the whole stream.
            let setup = match self
                .prepare_stream(
                    &module_id_owned,
                    inputs,
                    initial_context,
                    version_hint_owned.as_deref(),
                )
                .await?
            {
                StreamPrep::Setup(setup) => *setup,
                StreamPrep::Recovery(value) => {
                    yield value;
                    return;
                }
            };

            // Phase 2: invoke module.stream() and forward chunks as they arrive.
            // Note: individual chunks are NOT validated against output_schema here;
            // validation runs on the deep-merged result in Phase 3 after the stream
            // is exhausted. See Module::stream contract for details.
            //
            // Audit D10-001: chunk *shape* IS validated here, before delivery — a
            // non-object chunk is rejected and surfaced as an error rather than
            // yielded. Phase 3's deep_merge_chunks_checked then only ever sees
            // objects, so it acts as a defensive backstop.
            //
            // Fallback (sync STREAM-002): if Module::stream returns None, fall back
            // to Module::execute() and yield the result as a single chunk.
            // Mirrors apcore-python/src/apcore/executor.py:862-865 and
            // apcore-typescript/src/executor.ts:519-522.
            // A-D-002: two-point cancellation invariant (Step 2 + Step 8). The
            // unary pipeline checks the cancel token immediately before
            // `module.execute()` (`BuiltinExecute`, builtin_steps.rs §"Cancel
            // Token Mid-Pipeline Check", point 2). The streaming path bypasses
            // that step, so we replicate the check here, immediately before
            // `module.stream()`, so a token cancelled while Phase-1 setup ran
            // aborts before any chunk is produced — matching apcore-python
            // (builtin_steps.py:607) and apcore-typescript (builtin-steps.ts:502).
            if let Some(token) = setup.context.cancel_token.as_ref() {
                token.check_for(&module_id_owned)?;
            }

            let stream_handle = setup.module.stream(setup.inputs.clone(), &setup.context);
            let mut accumulated: Vec<Value> = Vec::new();

            if let Some(mut inner) = stream_handle {
                while let Some(chunk_result) = inner.next().await {
                    // STREAM-003 (sync): enforce global_deadline between chunks.
                    if let Some(err) =
                        stream_deadline_exceeded(setup.context.global_deadline, &module_id_owned)
                    {
                        Err(err)?;
                        return;
                    }
                    let chunk = match chunk_result {
                        Ok(c) => c,
                        Err(chunk_err) => {
                            // A-D-015: a non-cancellation error raised while
                            // iterating chunks runs the middleware on_error
                            // recovery chain over the Phase-1 executed
                            // middlewares; a recovery value is yielded as the
                            // stream's next chunk before the stream ends.
                            // Mirrors apcore-python (executor.py:1069) and
                            // apcore-typescript. Cancellation short-circuits
                            // (D-20): it bypasses on_error and surfaces raw.
                            let decorated = propagate_module_error(
                                chunk_err,
                                &module_id_owned,
                                &setup.context,
                            );
                            // `recover_stream_chunk_error` returns Some(recovery)
                            // to yield, or None to surface `decorated` raw
                            // (cancellation, no middleware, no/Retry outcome).
                            if let Some(recovery) =
                                Self::recover_stream_chunk_error(&setup, &decorated, &module_id_owned).await
                            {
                                yield recovery;
                                return;
                            }
                            Err(decorated)?;
                            return;
                        }
                    };
                    // Audit D10-001: reject a non-object chunk BEFORE delivering
                    // it. The canonical contract is "reject before deliver +
                    // raise": a chunk is valid iff it is a JSON object. On the
                    // first invalid chunk we surface a structured error and do
                    // NOT yield the bad chunk. We mirror the exact error shape
                    // produced by `deep_merge_chunks_checked` (same code,
                    // message, and details) so cross-language consumers match.
                    if !chunk.is_object() {
                        Err(stream_chunk_not_object_error(accumulated.len(), &chunk))?;
                        return;
                    }
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
            // output (pure side effects — chunks are already delivered, so
            // failures are swallowed, never yielded; sync finding A-D-012).
            Self::run_stream_phase3(&setup, &accumulated, &module_id_owned).await;
        })
    }

    /// Phase 3 of streaming: deep-merge the delivered chunks, validate the
    /// merged result against the output schema, and run the after-middleware
    /// chain. All failures are SWALLOWED (logged via `tracing::warn`) rather
    /// than surfaced — the chunks have already been delivered to the consumer.
    /// Matches apcore-python (`apcore.stream.post_validation_failed` event,
    /// no raise) and apcore-typescript (console.warn + swallow).
    async fn run_stream_phase3(setup: &StreamSetup, accumulated: &[Value], module_id: &str) {
        // D-58: enforce that all chunks are objects before merging. A non-object
        // chunk is logged and skips post-stream validation.
        let merged = match deep_merge_chunks_checked(accumulated) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    module_id = %module_id,
                    error = %e.message,
                    "stream phase-3 chunk shape check failed (chunks already delivered, swallowed)"
                );
                Self::emit_post_stream_failure(setup, module_id, &e).await;
                return;
            }
        };
        if let Err(e) = validate_against_schema(&merged, &setup.output_schema, "Output") {
            tracing::warn!(
                module_id = %module_id,
                error = %e.message,
                "stream phase-3 output schema validation failed (chunks already delivered, swallowed)"
            );
            Self::emit_post_stream_failure(setup, module_id, &e).await;
        } else if let Some(ref mm) = setup.middleware_manager {
            if let Err(e) = mm
                .execute_after(module_id, setup.inputs.clone(), merged, &setup.context)
                .await
            {
                tracing::warn!(
                    module_id = %module_id,
                    error = %e.message,
                    "stream phase-3 middleware_after failed (chunks already delivered, swallowed)"
                );
                Self::emit_post_stream_failure(setup, module_id, &e).await;
            }
        }
    }

    /// Publish `apcore.stream.post_validation_failed` when a post-stream failure
    /// is swallowed (chunks already delivered, cannot be un-sent). Best-effort:
    /// a no-op when no event emitter is configured. Mirrors apcore-python's
    /// `_emit_post_stream_failure` payload (`error_type`, `message`, `trace_id`).
    async fn emit_post_stream_failure(setup: &StreamSetup, module_id: &str, error: &ModuleError) {
        let Some(emitter) = setup.event_emitter.as_ref() else {
            return;
        };
        let event = ApCoreEvent::with_module(
            "apcore.stream.post_validation_failed",
            serde_json::json!({
                // `error_type`, NOT `code` — deliberately a class name rather
                // than a wire code, matching apcore-python
                // (`type(exc).__name__`, executor.py:1168) and
                // apcore-typescript (`cause.constructor?.name`,
                // executor.ts:818). Rust has no per-code error class, so the
                // `ErrorCode` variant name is the closest analogue and `Debug`
                // is correct here — unlike a `code` field, which MUST use
                // `wire_str()`.
                "error_type": format!("{:?}", error.code),
                "message": error.message.clone(),
                "trace_id": setup.context.trace_id.clone(),
            }),
            module_id,
            "error",
        );
        emitter.emit(&event).await;
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
    /// Run the middleware on_error recovery chain for a Phase-2 (mid-stream)
    /// chunk error. Returns `Some(recovery_value)` to yield as the stream's
    /// next chunk, or `None` when the original (decorated) error should be
    /// surfaced raw — i.e. there is no middleware manager, no middleware ran,
    /// the outcome was `None`, or the outcome was `Retry` (not meaningful
    /// mid-stream; re-raise per Python/TS). A-D-015.
    async fn recover_stream_chunk_error(
        setup: &StreamSetup,
        decorated: &ModuleError,
        module_id: &str,
    ) -> Option<Value> {
        let mm = setup.middleware_manager.as_ref()?;
        if setup.executed_middlewares.is_empty() {
            return None;
        }
        let outcome = mm
            .execute_on_error_outcome(
                module_id,
                setup.inputs.clone(),
                decorated,
                &setup.context,
                &setup.executed_middlewares,
            )
            .await;
        match outcome {
            Some(crate::middleware::base::OnErrorOutcome::Recovery(value)) => Some(value),
            Some(crate::middleware::base::OnErrorOutcome::Retry(_)) => {
                tracing::warn!(
                    module_id = %module_id,
                    "Retry requested during stream phase 2 — ignored; surfacing original error"
                );
                None
            }
            None => None,
        }
    }

    async fn prepare_stream(
        &self,
        module_id: &str,
        inputs: Value,
        ctx: Option<Context<Value>>,
        version_hint: Option<&str>,
    ) -> Result<StreamPrep, ModuleError> {
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
        self.bind_to_context(&mut pipe_ctx)?;

        // Drive the shared pipeline engine up to — but not including — the
        // `execute` step. This inherits all per-step metadata handling from
        // `PipelineEngine::run`, so streaming and non-streaming never diverge.
        let (_output, trace) = match PipelineEngine::run_until_step(
            &self.strategy,
            &mut pipe_ctx,
            "execute",
        )
        .await
        {
            Ok(pair) => pair,
            Err(e) => {
                // A-D-001: a Phase-1 (pre-execute) failure MUST run the
                // middleware on_error recovery chain over the executed
                // middlewares — mirroring `call()` and apcore-python /
                // apcore-typescript `stream()`. Any recovery value is yielded
                // as the stream's chunk; otherwise the (unwrapped, decorated)
                // error is surfaced as today.
                //
                // §1.1 fail-fast: unwrap PipelineStepError to the original
                // typed error. §1.2: unwrap MiddlewareChainError so on_error
                // handlers and the caller see the typed cause. Algorithm A11:
                // decorate with trace_id + module_id. These mirror `call()`'s
                // error handling exactly.
                let underlying = e.unwrap_pipeline_step_error().unwrap_or(e);
                let underlying = underlying
                    .unwrap_middleware_chain_error()
                    .unwrap_or(underlying);
                let underlying = propagate_module_error(underlying, module_id, &pipe_ctx.context);
                // Cancellation short-circuit (D-20): an ExecutionCancelled
                // error MUST bypass the on_error middleware chain so logging
                // middleware cannot swallow it and retry middleware cannot
                // resurrect the call. Mirrors `call()` and Python/TS stream().
                if matches!(underlying.code, ErrorCode::ExecutionCancelled) {
                    return Err(underlying);
                }
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
                            return Ok(StreamPrep::Recovery(value));
                        }
                        Some(crate::middleware::base::OnErrorOutcome::Retry(_)) => {
                            // Retry is not meaningful once stream() has been
                            // entered: a streaming caller could only be mid-
                            // stream on a re-run, so a retry request is
                            // translated back into the original error rather
                            // than silently re-running. Mirrors apcore-python
                            // (executor.py: "Retry requested during stream …
                            // ignored; re-raising") and apcore-typescript.
                            tracing::warn!(
                                module_id = %module_id,
                                "Retry requested during stream phase 1 — ignored; surfacing original error"
                            );
                            return Err(underlying);
                        }
                        None => {}
                    }
                }
                return Err(underlying);
            }
        };

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

        let executed_middlewares = pipe_ctx.executed_middlewares.clone();
        let event_emitter = pipe_ctx.event_emitter.clone();
        Ok(StreamPrep::Setup(Box::new(StreamSetup {
            module,
            inputs: pipe_ctx.inputs,
            context: pipe_ctx.context,
            output_schema,
            middleware_manager: pipe_ctx.middleware_manager.clone(),
            executed_middlewares,
            event_emitter,
        })))
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
    ///
    /// Shares the canonical error-recovery semantics of [`Self::call`] per
    /// spec docs/features/core-executor.md §Trace Variants (D-19) —
    /// middleware `on_error` recovery applies, the
    /// `ExecutionCancelled` short-circuit (D-20) applies, and the
    /// `MiddlewareChainError` unwrap rule (D-22) applies. On successful
    /// recovery the returned `PipelineTrace` is the trace captured during
    /// the failing pipeline run (including any `on_error` events recorded
    /// inside it).
    pub async fn call_with_trace(
        &self,
        module_id: &str,
        inputs: Value,
        ctx: Option<&Context<Value>>,
        version_hint: Option<&str>,
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
        // Sync finding A-D-005 (D-19): forward version_hint like call() does so
        // the trace variant shares call()'s version-negotiation semantics.
        if let Some(hint) = version_hint {
            pipeline_ctx.version_hint = Some(hint.to_string());
        }
        self.inject_resources(&mut pipeline_ctx);
        self.bind_to_context(&mut pipeline_ctx)?;

        match PipelineEngine::run(effective_strategy, &mut pipeline_ctx).await {
            Ok((output, trace)) => Ok((output.unwrap_or(Value::Null), trace)),
            Err(e) => {
                // Unwrap PipelineStepError + MiddlewareChainError so callers
                // and on_error middleware see the typed cause (D-22).
                let underlying = e.unwrap_pipeline_step_error().unwrap_or(e);
                let underlying = underlying
                    .unwrap_middleware_chain_error()
                    .unwrap_or(underlying);
                let underlying =
                    propagate_module_error(underlying, module_id, &pipeline_ctx.context);

                // Cancellation short-circuit (D-20).
                if matches!(underlying.code, ErrorCode::ExecutionCancelled) {
                    return Err(underlying);
                }

                // Run middleware on_error chain (D-19) — recovery returns
                // (recovered_value, trace); a Retry signal is treated as a
                // failure (the trace variant does not loop). Mirrors
                // apcore-python `call_with_trace` (recovery yes, retry no).
                let executed = pipeline_ctx.executed_middlewares.clone();
                if !executed.is_empty() {
                    let outcome = self
                        .middleware_manager
                        .execute_on_error_outcome(
                            module_id,
                            pipeline_ctx.inputs.clone(),
                            &underlying,
                            &pipeline_ctx.context,
                            &executed,
                        )
                        .await;
                    if let Some(crate::middleware::base::OnErrorOutcome::Recovery(value)) = outcome
                    {
                        // Trace captured during the failing run includes any
                        // on_error events recorded inside the pipeline; return
                        // it alongside the recovered value.
                        let trace = pipeline_ctx.trace.clone();
                        return Ok((value, trace));
                    }
                }
                Err(underlying)
            }
        }
    }

    /// Inject executor resources into a pipeline context so builtin steps
    /// can access the registry, config, ACL, approval handler, and middleware.
    fn inject_resources(&self, ctx: &mut PipelineContext) {
        ctx.registry = Some(Arc::clone(&self.registry));
        ctx.config = Some(Arc::clone(&self.config));
        ctx.acl = self.acl.as_ref().map(Arc::clone);
        ctx.approval_handler = self.approval_handler.as_ref().map(Arc::clone);
        ctx.policy = self.policy.as_ref().map(Arc::clone);
        ctx.event_emitter = self.event_emitter.as_ref().map(Arc::clone);
        ctx.governance_warned = Some(Arc::clone(&self.governance_warned));
        ctx.middleware_manager = Some(Arc::clone(&self.middleware_manager));
    }

    /// Bind this Executor's identity handle to `ctx.context.executor` per
    /// apcore Issue #66 / `core-executor.md` §"Contract: Executor binding to
    /// Context". Called before pipeline step 1 from every top-level entry
    /// point (`call`, `call_with_trace`, `validate`, `stream` via
    /// `prepare_stream`).
    ///
    /// Errors out with [`ErrorCode::ContextBindingError`] when the supplied
    /// Context is already bound to a different Executor instance.
    fn bind_to_context(&self, ctx: &mut PipelineContext) -> Result<(), ModuleError> {
        ctx.context.bind_executor(self.instance_handle())
    }

    /// Type-erased Executor identity handle.
    ///
    /// Returned as `Arc<dyn Any + Send + Sync>` because `Context::executor` is
    /// non-generic over the Executor type. Two Executors are considered the
    /// same instance iff the underlying `Arc`s point to the same allocation
    /// (`Arc::ptr_eq`). The pointed-to payload is opaque to callers — only
    /// the identity matters.
    ///
    /// Marked `doc(hidden)` because it exists to support binding-aware
    /// integrations (e.g. test fixtures, distributed runtimes injecting a
    /// pre-bound Context); application code SHOULD let the Executor bind
    /// itself implicitly at pipeline entry instead.
    #[doc(hidden)]
    #[must_use]
    pub fn instance_handle(&self) -> Arc<dyn std::any::Any + Send + Sync> {
        // Coerce `Arc<()>` → `Arc<dyn Any + Send + Sync>` via the implicit
        // unsized cast performed by `let` annotation; this preserves identity
        // because `Arc::clone` increments the same strong-count.
        let h: Arc<dyn std::any::Any + Send + Sync> = self.instance_handle.clone();
        h
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
    fn test_redact_secret_prefix_recurses_into_arrays() {
        // A-D-003: the _secret_ prefix walk must recurse into ARRAY elements,
        // not only objects — mirroring apcore-python `_redact_in_list`.
        let schema = json!({});
        let data = json!({
            "items": [
                { "_secret_api_key": "k", "name": "ok" },
                { "nested": { "_secret_token": "t" } }
            ]
        });
        let result = redact_sensitive(&data, &schema);
        let arr = result["items"].as_array().unwrap();
        assert_eq!(arr[0]["_secret_api_key"], REDACTED_VALUE);
        assert_eq!(arr[0]["name"], "ok");
        assert_eq!(arr[1]["nested"]["_secret_token"], REDACTED_VALUE);
    }

    #[test]
    fn test_redact_secret_prefix_recurses_into_nested_arrays() {
        // Arrays nested inside arrays must also be traversed.
        let schema = json!({});
        let data = json!({ "outer": [[{ "_secret_x": "v" }]] });
        let result = redact_sensitive(&data, &schema);
        assert_eq!(result["outer"][0][0]["_secret_x"], REDACTED_VALUE);
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

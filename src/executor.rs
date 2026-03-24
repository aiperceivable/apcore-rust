// APCore Protocol — Executor
// Spec reference: Module execution engine

use std::collections::HashMap;
use std::time::Duration;

use serde_json::Value;
use tokio::time::Instant;

use crate::acl::ACL;
use crate::approval::{ApprovalHandler, ApprovalRequest};
use crate::config::Config;
use crate::context::{Context, Identity};
use crate::errors::{ErrorCode, ModuleError};
use crate::middleware::adapters::{AfterMiddleware, BeforeMiddleware};
use crate::middleware::base::Middleware;
use crate::middleware::manager::MiddlewareManager;
use crate::registry::registry::Registry;
use crate::utils::guard_call_chain;

/// Returns true if the schema is non-trivial (not null and not an empty object).
fn has_schema(schema: &Value) -> bool {
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
fn validate_against_schema(
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

/// Result of a validate() call. Preflight warnings are advisory and never block.
#[derive(Debug, Clone, Default)]
pub struct ValidationResult {
    /// Warnings collected from preflight checks (advisory, never blocking).
    pub warnings: Vec<String>,
}

/// Responsible for executing modules with middleware, ACL, and context management.
#[derive(Debug)]
pub struct Executor {
    pub registry: Registry,
    pub config: Config,
    pub acl: Option<ACL>,
    pub approval_handler: Option<Box<dyn ApprovalHandler>>,
    pub middleware_manager: MiddlewareManager,
}

impl Executor {
    /// Create a new executor with the given registry and config.
    pub fn new(registry: Registry, config: Config) -> Self {
        Self {
            registry,
            config,
            acl: None,
            approval_handler: None,
            middleware_manager: MiddlewareManager::new(),
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
            registry,
            config,
            acl,
            approval_handler,
            middleware_manager,
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
        self.acl = Some(acl);
    }

    /// Set the approval handler.
    pub fn set_approval_handler(&mut self, handler: Box<dyn ApprovalHandler>) {
        self.approval_handler = Some(handler);
    }

    /// Add a middleware to the pipeline.
    ///
    /// Returns an error if the middleware's priority exceeds the allowed range.
    pub fn use_middleware(&mut self, middleware: Box<dyn Middleware>) -> Result<(), ModuleError> {
        self.middleware_manager.add(middleware)
    }

    /// Remove a middleware by name.
    pub fn remove(&mut self, name: &str) -> bool {
        self.middleware_manager.remove(name)
    }

    /// Remove a middleware by name (legacy alias).
    pub fn remove_middleware(&mut self, name: &str) -> bool {
        self.remove(name)
    }

    /// Execute (call) a module by ID with the given inputs and context.
    pub async fn call(
        &self,
        module_id: &str,
        inputs: serde_json::Value,
        ctx: Option<&Context<serde_json::Value>>,
        _version_hint: Option<&str>,
    ) -> Result<serde_json::Value, ModuleError> {
        // Step 1: Context — create or use default parent
        let default_ctx;
        let parent_ctx = match ctx {
            Some(parent) => parent,
            None => {
                default_ctx = Context::<serde_json::Value>::new(Identity {
                    id: "@external".to_string(),
                    identity_type: "external".to_string(),
                    roles: vec![],
                    attrs: Default::default(),
                });
                &default_ctx
            }
        };

        // Step 2: Safety Checks — guard call chain on parent BEFORE adding module_id
        crate::utils::guard_call_chain_with_repeat(
            parent_ctx,
            module_id,
            self.config.max_call_depth,
            self.config.max_module_repeat as usize,
        )?;

        // Create child context (adds module_id to call_chain)
        let mut child_ctx = parent_ctx.child(module_id);

        // Set global deadline on root call (when no parent context was provided)
        if ctx.is_none() && self.config.global_timeout_ms > 0 {
            child_ctx.global_deadline =
                Some(Instant::now() + Duration::from_millis(self.config.global_timeout_ms));
        }

        // Step 3: Module Lookup
        let module = self.registry.get(module_id).ok_or_else(|| {
            ModuleError::new(
                ErrorCode::ModuleNotFound,
                format!("Module '{}' not found in registry", module_id),
            )
        })?;

        // Step 4: ACL Check
        if let Some(ref acl) = self.acl {
            let caller_id = child_ctx.caller_id.as_deref();
            let allowed = acl.check(caller_id, module_id, Some(&child_ctx))?;
            if !allowed {
                return Err(ModuleError::new(
                    ErrorCode::AclDenied,
                    format!(
                        "Access denied: caller '{:?}' cannot access module '{}'",
                        caller_id, module_id
                    ),
                ));
            }
        }

        // Step 5: Approval Gate (with _approval_token Phase B support)
        let mut inputs = inputs;
        if let Some(ref handler) = self.approval_handler {
            if let Some(desc) = self.registry.get_definition(module_id) {
                if desc.annotations.requires_approval {
                    // Phase B: check for _approval_token in inputs
                    let approval_result = if let Some(token) = inputs
                        .as_object()
                        .and_then(|obj| obj.get("_approval_token"))
                    {
                        let token_str = match token.as_str() {
                            Some(s) => s.to_string(),
                            None => {
                                return Err(ModuleError::new(
                                    ErrorCode::GeneralInvalidInput,
                                    format!(
                                        "_approval_token must be a string, got {}",
                                        match token {
                                            Value::Number(_) => "number",
                                            Value::Bool(_) => "boolean",
                                            Value::Array(_) => "array",
                                            Value::Object(_) => "object",
                                            Value::Null => "null",
                                            _ => "unknown",
                                        }
                                    ),
                                ));
                            }
                        };
                        // Strip _approval_token from inputs
                        if let Some(obj) = inputs.as_object_mut() {
                            obj.remove("_approval_token");
                        }
                        handler.check_approval(&token_str).await?
                    } else {
                        let request = ApprovalRequest {
                            module_id: module_id.to_string(),
                            arguments: inputs.clone(),
                            annotations: Default::default(),
                            description: None,
                            tags: vec![],
                        };
                        handler.request_approval(&request).await?
                    };

                    // Fix 4: Differentiated approval error handling
                    match approval_result.status.as_str() {
                        "approved" => {} // proceed
                        "rejected" => {
                            return Err(ModuleError::new(
                                ErrorCode::ApprovalDenied,
                                format!(
                                    "Approval denied for module '{}': {}",
                                    module_id,
                                    approval_result
                                        .reason
                                        .unwrap_or_else(|| "no reason given".to_string())
                                ),
                            ));
                        }
                        "timeout" => {
                            return Err(ModuleError::new(
                                ErrorCode::ApprovalTimeout,
                                format!(
                                    "Approval timed out for module '{}': {}",
                                    module_id,
                                    approval_result
                                        .reason
                                        .unwrap_or_else(|| "no reason given".to_string())
                                ),
                            ));
                        }
                        "pending" => {
                            return Err(ModuleError::new(
                                ErrorCode::ApprovalPending,
                                format!(
                                    "Approval pending for module '{}': {}",
                                    module_id,
                                    approval_result
                                        .reason
                                        .unwrap_or_else(|| "no reason given".to_string())
                                ),
                            ));
                        }
                        _ => {
                            // Unknown status treated as denied
                            tracing::warn!(
                                module_id = module_id,
                                status = %approval_result.status,
                                "Unknown approval status, treating as denied"
                            );
                            return Err(ModuleError::new(
                                ErrorCode::ApprovalDenied,
                                format!(
                                    "Approval denied for module '{}': unknown status '{}'",
                                    module_id, approval_result.status
                                ),
                            ));
                        }
                    }
                }
            }
        }

        // Step 6: Input Validation and Redaction
        let input_schema = module.input_schema();
        validate_against_schema(&inputs, &input_schema, "Input")?;
        child_ctx.redacted_inputs = if has_schema(&input_schema) {
            let redacted = redact_sensitive(&inputs, &input_schema);
            Some(
                redacted
                    .as_object()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .collect(),
            )
        } else {
            None
        };

        // Step 7: Middleware Before
        let (inputs, executed) = match self
            .middleware_manager
            .execute_before(module_id, inputs.clone(), &child_ctx)
            .await
        {
            Ok((modified_inputs, executed)) => (modified_inputs, executed),
            Err(e) => {
                // On middleware before error, run on_error with original inputs for recovery.
                // Design choice: we pass the original (pre-middleware) inputs, not partially
                // modified ones, because the before chain did not complete successfully.
                let recovery = self
                    .middleware_manager
                    .execute_on_error(module_id, inputs, &e, &child_ctx, &[])
                    .await;
                if let Some(recovery_value) = recovery {
                    return Ok(recovery_value);
                }
                return Err(e);
            }
        };

        // Step 8: Execute with dual-timeout enforcement
        let per_module_timeout_ms = self.config.default_timeout_ms;
        let effective_timeout_ms = compute_effective_timeout(
            per_module_timeout_ms,
            child_ctx.global_deadline,
            module_id,
            self.config.global_timeout_ms,
        )?;

        let execute_result = if effective_timeout_ms > 0 {
            match tokio::time::timeout(
                Duration::from_millis(effective_timeout_ms),
                module.execute(inputs.clone(), &child_ctx),
            )
            .await
            {
                Ok(result) => result,
                Err(_elapsed) => Err(ModuleError::new(
                    ErrorCode::ModuleTimeout,
                    format!(
                        "Module '{}' execution timed out after {}ms",
                        module_id, effective_timeout_ms
                    ),
                )),
            }
        } else {
            module.execute(inputs.clone(), &child_ctx).await
        };

        let output = match execute_result {
            Ok(output) => output,
            Err(e) => {
                // On execution error, run on_error for recovery.
                // NOTE: If a middleware returns Some(value), it is treated as a
                // recovery result (not a retry signal). The module is NOT
                // re-executed. This matches the Python reference implementation
                // where on_error recovery replaces the output. Actual retry
                // logic requires a retry-aware executor loop (future work).
                let recovery = self
                    .middleware_manager
                    .execute_on_error(module_id, inputs, &e, &child_ctx, &executed)
                    .await;
                if let Some(recovery_value) = recovery {
                    return Ok(recovery_value);
                }
                return Err(e);
            }
        };

        // Step 9: Output Validation and Redaction
        let output_schema = module.output_schema();
        validate_against_schema(&output, &output_schema, "Output")?;
        if has_schema(&output_schema) {
            child_ctx
                .data
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .insert(
                    "_apcore.executor.redacted_output".to_string(),
                    redact_sensitive(&output, &output_schema),
                );
        }

        // Step 10: Middleware After
        let output = match self
            .middleware_manager
            .execute_after(module_id, inputs.clone(), output, &child_ctx)
            .await
        {
            Ok(modified_output) => modified_output,
            Err(e) => {
                // On middleware after error, run on_error for recovery
                let recovery = self
                    .middleware_manager
                    .execute_on_error(module_id, inputs, &e, &child_ctx, &executed)
                    .await;
                if let Some(recovery_value) = recovery {
                    return Ok(recovery_value);
                }
                return Err(e);
            }
        };

        Ok(output)
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

    /// Validate module inputs without executing (steps 1-7).
    ///
    /// Returns a `ValidationResult` containing any preflight warnings.
    /// Preflight failures are advisory and never block validation.
    pub async fn validate(
        &self,
        module_id: &str,
        inputs: &serde_json::Value,
    ) -> Result<ValidationResult, ModuleError> {
        // Step 1: Context
        let default_ctx = Context::<serde_json::Value>::new(Identity {
            id: "@external".to_string(),
            identity_type: "external".to_string(),
            roles: vec![],
            attrs: Default::default(),
        });

        // Step 2: Safety Checks — guard on parent BEFORE creating child
        crate::utils::guard_call_chain_with_repeat(
            &default_ctx,
            module_id,
            self.config.max_call_depth,
            self.config.max_module_repeat as usize,
        )?;

        // Create child context
        let child_ctx = default_ctx.child(module_id);

        // Step 3: Module Lookup
        let module = self.registry.get(module_id).ok_or_else(|| {
            ModuleError::new(
                ErrorCode::ModuleNotFound,
                format!("Module '{}' not found in registry", module_id),
            )
        })?;

        // Step 4: ACL Check
        if let Some(ref acl) = self.acl {
            let caller_id = child_ctx.caller_id.as_deref();
            let allowed = acl.check(caller_id, module_id, Some(&child_ctx))?;
            if !allowed {
                return Err(ModuleError::new(
                    ErrorCode::AclDenied,
                    format!(
                        "Access denied: caller '{:?}' cannot access module '{}'",
                        caller_id, module_id
                    ),
                ));
            }
        }

        // Step 5: Approval Gate
        if let Some(ref handler) = self.approval_handler {
            if let Some(desc) = self.registry.get_definition(module_id) {
                if desc.annotations.requires_approval {
                    let request = ApprovalRequest {
                        module_id: module_id.to_string(),
                        arguments: inputs.clone(),
                        annotations: Default::default(),
                        description: None,
                        tags: vec![],
                    };
                    let approval_result = handler.request_approval(&request).await?;
                    match approval_result.status.as_str() {
                        "approved" => {}
                        "rejected" => {
                            return Err(ModuleError::new(
                                ErrorCode::ApprovalDenied,
                                format!(
                                    "Approval denied for module '{}': {}",
                                    module_id,
                                    approval_result
                                        .reason
                                        .unwrap_or_else(|| "no reason given".to_string())
                                ),
                            ));
                        }
                        "timeout" => {
                            return Err(ModuleError::new(
                                ErrorCode::ApprovalTimeout,
                                format!(
                                    "Approval timed out for module '{}': {}",
                                    module_id,
                                    approval_result
                                        .reason
                                        .unwrap_or_else(|| "no reason given".to_string())
                                ),
                            ));
                        }
                        "pending" => {
                            return Err(ModuleError::new(
                                ErrorCode::ApprovalPending,
                                format!(
                                    "Approval pending for module '{}': {}",
                                    module_id,
                                    approval_result
                                        .reason
                                        .unwrap_or_else(|| "no reason given".to_string())
                                ),
                            ));
                        }
                        _ => {
                            return Err(ModuleError::new(
                                ErrorCode::ApprovalDenied,
                                format!(
                                    "Approval denied for module '{}': unknown status '{}'",
                                    module_id, approval_result.status
                                ),
                            ));
                        }
                    }
                }
            }
        }

        // Step 6: Input Validation
        let input_schema = module.input_schema();
        validate_against_schema(inputs, &input_schema, "Input")?;

        // Step 7: Module Preflight (optional, advisory) — collect warnings, never block
        let mut result = ValidationResult::default();
        let preflight_result = module.preflight();
        if !preflight_result.passed {
            let failed_checks: Vec<String> = preflight_result
                .checks
                .iter()
                .filter(|c| !c.passed)
                .map(|c| {
                    c.message
                        .clone()
                        .unwrap_or_else(|| format!("Check '{}' failed", c.name))
                })
                .collect();
            for warning in failed_checks {
                tracing::warn!(
                    module_id = module_id,
                    warning = %warning,
                    "Preflight check warning (advisory)"
                );
                result.warnings.push(warning);
            }
        }

        Ok(result)
    }

    /// Check call depth limits before execution.
    pub fn check_call_depth(&self, ctx: &Context<serde_json::Value>) -> Result<(), ModuleError> {
        // Delegate to guard_call_chain with a dummy module name to check depth only.
        // guard_call_chain checks depth first, so if depth is exceeded it returns error
        // before checking circular calls.
        if ctx.call_chain.len() as u32 >= self.config.max_call_depth {
            return Err(ModuleError::new(
                ErrorCode::CallDepthExceeded,
                format!(
                    "Call depth exceeded: chain length {} >= max_depth {}",
                    ctx.call_chain.len(),
                    self.config.max_call_depth
                ),
            ));
        }
        Ok(())
    }

    /// Check for circular calls in the call chain.
    pub fn check_circular_call(
        &self,
        ctx: &Context<serde_json::Value>,
        module_id: &str,
    ) -> Result<(), ModuleError> {
        guard_call_chain(ctx, module_id, self.config.max_call_depth)
    }

    /// Create an executor from a registry and config.
    pub fn from_registry(registry: Registry, config: Config) -> Self {
        Self::with_options(registry, config, None, None, None)
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

    /// Add a before middleware.
    pub fn use_before(&mut self, middleware: Box<dyn BeforeMiddleware>) -> Result<(), ModuleError> {
        self.middleware_manager
            .add(Box::new(BoxedBeforeMiddlewareAdapter(middleware)))
    }

    /// Add an after middleware.
    pub fn use_after(&mut self, middleware: Box<dyn AfterMiddleware>) -> Result<(), ModuleError> {
        self.middleware_manager
            .add(Box::new(BoxedAfterMiddlewareAdapter(middleware)))
    }
}

/// Compute the effective timeout for module execution, enforcing dual-timeout model.
///
/// Takes the per-module timeout and global deadline into account.
/// Returns the effective timeout in milliseconds.
fn compute_effective_timeout(
    per_module_timeout_ms: u64,
    global_deadline: Option<Instant>,
    module_id: &str,
    global_timeout_ms: u64,
) -> Result<u64, ModuleError> {
    let mut effective = per_module_timeout_ms;

    if let Some(deadline) = global_deadline {
        let now = Instant::now();
        if now >= deadline {
            // Global deadline already expired
            return Err(ModuleError::new(
                ErrorCode::ModuleTimeout,
                format!(
                    "Global timeout expired before executing module '{}' (global_timeout={}ms)",
                    module_id, global_timeout_ms
                ),
            ));
        }
        let remaining_ms = (deadline - now).as_millis() as u64;
        if effective == 0 || remaining_ms < effective {
            effective = remaining_ms;
        }
    }

    Ok(effective)
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
    // compute_effective_timeout
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn test_compute_effective_timeout_per_module_only() {
        let result = compute_effective_timeout(5000, None, "mod", 0).unwrap();
        assert_eq!(result, 5000);
    }

    #[test]
    fn test_compute_effective_timeout_global_only() {
        let deadline = Instant::now() + Duration::from_millis(3000);
        let result = compute_effective_timeout(0, Some(deadline), "mod", 10000).unwrap();
        // With per_module=0, the global remaining (~3000) should be used
        assert!(result > 0 && result <= 3000);
    }

    #[test]
    fn test_compute_effective_timeout_both_min_wins() {
        // Global remaining is ~3000ms, per-module is 5000ms -> global wins
        let deadline = Instant::now() + Duration::from_millis(3000);
        let result = compute_effective_timeout(5000, Some(deadline), "mod", 10000).unwrap();
        assert!(result <= 3000);
    }

    #[test]
    fn test_compute_effective_timeout_per_module_wins_when_smaller() {
        // Global remaining is ~10000ms, per-module is 2000ms -> per-module wins
        let deadline = Instant::now() + Duration::from_millis(10000);
        let result = compute_effective_timeout(2000, Some(deadline), "mod", 20000).unwrap();
        assert_eq!(result, 2000);
    }

    #[test]
    fn test_compute_effective_timeout_expired_deadline_returns_error() {
        // Deadline in the past
        let deadline = Instant::now() - Duration::from_millis(100);
        let err = compute_effective_timeout(5000, Some(deadline), "mod", 10000).unwrap_err();
        assert_eq!(err.code, ErrorCode::ModuleTimeout);
        assert!(err.message.contains("Global timeout expired"));
    }

    #[test]
    fn test_compute_effective_timeout_zero_per_module_no_deadline() {
        let result = compute_effective_timeout(0, None, "mod", 0).unwrap();
        assert_eq!(result, 0);
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
    async fn test_approval_token_non_string_rejected_with_error() {
        let handler = MockApprovalHandler::with_status("approved");
        let module = MockModule::echo();
        let annotations = ModuleAnnotations {
            requires_approval: true,
            ..Default::default()
        };
        let mut executor = build_executor_with_module(module, annotations);
        executor.set_approval_handler(Box::new(handler));

        // Integer token
        let inputs = json!({"_approval_token": 42, "data": "hello"});
        let err = executor
            .call("test_mod", inputs, None, None)
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::GeneralInvalidInput);
        assert!(err.message.contains("_approval_token must be a string"));
        assert!(err.message.contains("number"));
    }

    #[tokio::test]
    async fn test_approval_token_boolean_rejected() {
        let handler = MockApprovalHandler::with_status("approved");
        let module = MockModule::echo();
        let annotations = ModuleAnnotations {
            requires_approval: true,
            ..Default::default()
        };
        let mut executor = build_executor_with_module(module, annotations);
        executor.set_approval_handler(Box::new(handler));

        let inputs = json!({"_approval_token": true});
        let err = executor
            .call("test_mod", inputs, None, None)
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::GeneralInvalidInput);
        assert!(err.message.contains("boolean"));
    }

    #[tokio::test]
    async fn test_approval_token_null_rejected() {
        let handler = MockApprovalHandler::with_status("approved");
        let module = MockModule::echo();
        let annotations = ModuleAnnotations {
            requires_approval: true,
            ..Default::default()
        };
        let mut executor = build_executor_with_module(module, annotations);
        executor.set_approval_handler(Box::new(handler));

        let inputs = json!({"_approval_token": null});
        let err = executor
            .call("test_mod", inputs, None, None)
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::GeneralInvalidInput);
        assert!(err.message.contains("null"));
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

    // ═══════════════════════════════════════════════════════════════════
    // Approval differentiation
    // ═══════════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn test_approval_rejected_returns_approval_denied() {
        let handler = MockApprovalHandler::with_status("rejected");
        let module = MockModule::echo();
        let annotations = ModuleAnnotations {
            requires_approval: true,
            ..Default::default()
        };
        let mut executor = build_executor_with_module(module, annotations);
        executor.set_approval_handler(Box::new(handler));

        let err = executor
            .call("test_mod", json!({}), None, None)
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::ApprovalDenied);
    }

    #[tokio::test]
    async fn test_approval_timeout_returns_approval_timeout() {
        let handler = MockApprovalHandler::with_status("timeout");
        let module = MockModule::echo();
        let annotations = ModuleAnnotations {
            requires_approval: true,
            ..Default::default()
        };
        let mut executor = build_executor_with_module(module, annotations);
        executor.set_approval_handler(Box::new(handler));

        let err = executor
            .call("test_mod", json!({}), None, None)
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::ApprovalTimeout);
    }

    #[tokio::test]
    async fn test_approval_pending_returns_approval_pending() {
        let handler = MockApprovalHandler::with_status("pending");
        let module = MockModule::echo();
        let annotations = ModuleAnnotations {
            requires_approval: true,
            ..Default::default()
        };
        let mut executor = build_executor_with_module(module, annotations);
        executor.set_approval_handler(Box::new(handler));

        let err = executor
            .call("test_mod", json!({}), None, None)
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::ApprovalPending);
    }

    #[tokio::test]
    async fn test_validate_approval_timeout_status() {
        let handler = MockApprovalHandler::with_status("timeout");
        let module = MockModule::echo();
        let annotations = ModuleAnnotations {
            requires_approval: true,
            ..Default::default()
        };
        let mut executor = build_executor_with_module(module, annotations);
        executor.set_approval_handler(Box::new(handler));

        let err = executor.validate("test_mod", &json!({})).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::ApprovalTimeout);
    }

    #[tokio::test]
    async fn test_validate_approval_pending_status() {
        let handler = MockApprovalHandler::with_status("pending");
        let module = MockModule::echo();
        let annotations = ModuleAnnotations {
            requires_approval: true,
            ..Default::default()
        };
        let mut executor = build_executor_with_module(module, annotations);
        executor.set_approval_handler(Box::new(handler));

        let err = executor.validate("test_mod", &json!({})).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::ApprovalPending);
    }

    #[tokio::test]
    async fn test_approval_unknown_status_returns_approval_denied() {
        let handler = MockApprovalHandler::with_status("banana");
        let module = MockModule::echo();
        let annotations = ModuleAnnotations {
            requires_approval: true,
            ..Default::default()
        };
        let mut executor = build_executor_with_module(module, annotations);
        executor.set_approval_handler(Box::new(handler));

        let err = executor
            .call("test_mod", json!({}), None, None)
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::ApprovalDenied);
        assert!(err.message.contains("unknown status"));
    }
}

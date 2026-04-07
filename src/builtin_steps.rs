// APCore Protocol — Built-in execution pipeline steps
// Spec reference: design-execution-pipeline.md (Section 3)

use std::time::Duration;

use async_trait::async_trait;

use crate::context::Identity;
use crate::errors::{ErrorCode, ModuleError};
use crate::executor::{has_schema, redact_sensitive, validate_against_schema};
use crate::pipeline::{ExecutionStrategy, PipelineContext, Step, StepResult};

// Macro for step metadata — execute is implemented manually per step.
macro_rules! step_meta {
    ($name:ident, $step_name:expr, $desc:expr, removable=$rm:expr, replaceable=$rp:expr, pure=$pure:expr) => {
        pub struct $name;

        impl $name {
            fn _name(&self) -> &str {
                $step_name
            }
            fn _description(&self) -> &str {
                $desc
            }
            fn _removable(&self) -> bool {
                $rm
            }
            fn _replaceable(&self) -> bool {
                $rp
            }
            fn _pure(&self) -> bool {
                $pure
            }
        }
    };
}

// Shared helper macro to implement the non-execute Step trait methods.
macro_rules! impl_step_meta {
    ($name:ident) => {
        fn name(&self) -> &str {
            self._name()
        }
        fn description(&self) -> &str {
            self._description()
        }
        fn removable(&self) -> bool {
            self._removable()
        }
        fn replaceable(&self) -> bool {
            self._replaceable()
        }
        fn pure(&self) -> bool {
            self._pure()
        }
    };
}

step_meta!(
    BuiltinContextCreation,
    "context_creation",
    "Create or inherit execution context",
    removable = false,
    replaceable = false,
    pure = true
);
step_meta!(
    BuiltinCallChainGuard,
    "call_chain_guard",
    "Validate call depth and module repeat limits",
    removable = true,
    replaceable = true,
    pure = true
);
step_meta!(
    BuiltinModuleLookup,
    "module_lookup",
    "Resolve module from registry by ID",
    removable = false,
    replaceable = false,
    pure = true
);
step_meta!(
    BuiltinACLCheck,
    "acl_check",
    "Enforce access control rules",
    removable = true,
    replaceable = true,
    pure = true
);
step_meta!(
    BuiltinApprovalGate,
    "approval_gate",
    "Handle human or AI approval flow",
    removable = true,
    replaceable = true,
    pure = false
);
step_meta!(
    BuiltinInputValidation,
    "input_validation",
    "Validate inputs against schema",
    removable = true,
    replaceable = true,
    pure = true
);
step_meta!(
    BuiltinMiddlewareBefore,
    "middleware_before",
    "Execute before-middleware chain",
    removable = true,
    replaceable = false,
    pure = false
);
step_meta!(
    BuiltinExecute,
    "execute",
    "Invoke the module with timeout",
    removable = false,
    replaceable = true,
    pure = false
);
step_meta!(
    BuiltinOutputValidation,
    "output_validation",
    "Validate outputs against schema",
    removable = true,
    replaceable = true,
    pure = true
);
step_meta!(
    BuiltinMiddlewareAfter,
    "middleware_after",
    "Execute after-middleware chain",
    removable = true,
    replaceable = false,
    pure = false
);
step_meta!(
    BuiltinReturnResult,
    "return_result",
    "Finalize and return output",
    removable = false,
    replaceable = false,
    pure = true
);

// ---------------------------------------------------------------------------
// Step implementations
// ---------------------------------------------------------------------------

#[async_trait]
impl Step for BuiltinContextCreation {
    impl_step_meta!(BuiltinContextCreation);
    fn provides(&self) -> &[&str] {
        &["context"]
    }

    async fn execute(&self, ctx: &mut PipelineContext) -> Result<StepResult, ModuleError> {
        // If the context has no caller_id, default to @external.
        if ctx.context.caller_id.is_none() {
            ctx.context = crate::context::Context::new(Identity::new(
                "@external".to_string(),
                "external".to_string(),
                vec![],
                Default::default(),
            ));
        }
        Ok(StepResult::continue_step())
    }
}

#[async_trait]
impl Step for BuiltinCallChainGuard {
    impl_step_meta!(BuiltinCallChainGuard);
    fn requires(&self) -> &[&str] {
        &["context"]
    }

    async fn execute(&self, ctx: &mut PipelineContext) -> Result<StepResult, ModuleError> {
        let config = ctx
            .config
            .as_ref()
            .expect("config must be injected into PipelineContext");
        crate::utils::guard_call_chain_with_repeat(
            &ctx.context,
            &ctx.module_id,
            config.executor.max_call_depth,
            config.executor.max_module_repeat as usize,
        )?;
        // Create child context (adds module_id to call_chain).
        ctx.context = ctx.context.child(&ctx.module_id);
        Ok(StepResult::continue_step())
    }
}

#[async_trait]
impl Step for BuiltinModuleLookup {
    impl_step_meta!(BuiltinModuleLookup);
    fn provides(&self) -> &[&str] {
        &["module"]
    }

    async fn execute(&self, ctx: &mut PipelineContext) -> Result<StepResult, ModuleError> {
        let registry = ctx
            .registry
            .as_ref()
            .expect("registry must be injected into PipelineContext");
        let module = registry.get(&ctx.module_id).ok_or_else(|| {
            ModuleError::new(
                ErrorCode::ModuleNotFound,
                format!("Module '{}' not found in registry", ctx.module_id),
            )
        })?;
        ctx.module = Some(module);
        Ok(StepResult::continue_step())
    }
}

#[async_trait]
impl Step for BuiltinACLCheck {
    impl_step_meta!(BuiltinACLCheck);
    fn requires(&self) -> &[&str] {
        &["context", "module"]
    }

    async fn execute(&self, ctx: &mut PipelineContext) -> Result<StepResult, ModuleError> {
        if let Some(ref acl) = ctx.acl {
            let caller_id = ctx.context.caller_id.as_deref();
            let allowed = acl.check(caller_id, &ctx.module_id, Some(&ctx.context))?;
            if !allowed {
                return Err(ModuleError::new(
                    ErrorCode::AclDenied,
                    format!(
                        "Access denied: caller '{:?}' cannot access module '{}'",
                        caller_id, ctx.module_id
                    ),
                ));
            }
        }
        Ok(StepResult::continue_step())
    }
}

#[async_trait]
impl Step for BuiltinApprovalGate {
    impl_step_meta!(BuiltinApprovalGate);
    fn requires(&self) -> &[&str] {
        &["context", "module"]
    }

    async fn execute(&self, ctx: &mut PipelineContext) -> Result<StepResult, ModuleError> {
        let handler = match ctx.approval_handler {
            Some(ref h) => h.clone(),
            None => return Ok(StepResult::continue_step()),
        };

        let registry = ctx
            .registry
            .as_ref()
            .expect("registry must be injected into PipelineContext");

        let desc = match registry.get_definition(&ctx.module_id) {
            Some(d) if d.annotations.requires_approval => d,
            _ => return Ok(StepResult::continue_step()),
        };
        let _ = desc; // used only for the requires_approval check above

        // Phase B: check for _approval_token in inputs.
        let approval_result = if let Some(token) = ctx
            .inputs
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
                                serde_json::Value::Number(_) => "number",
                                serde_json::Value::Bool(_) => "boolean",
                                serde_json::Value::Array(_) => "array",
                                serde_json::Value::Object(_) => "object",
                                serde_json::Value::Null => "null",
                                _ => "unknown",
                            }
                        ),
                    ));
                }
            };
            // Strip _approval_token from inputs.
            if let Some(obj) = ctx.inputs.as_object_mut() {
                obj.remove("_approval_token");
            }
            handler.check_approval(&token_str).await?
        } else {
            let request = crate::approval::ApprovalRequest {
                module_id: ctx.module_id.clone(),
                arguments: ctx.inputs.clone(),
                context: Some(ctx.context.clone()),
                annotations: Default::default(),
                description: None,
                tags: vec![],
            };
            handler.request_approval(&request).await?
        };

        match approval_result.status.as_str() {
            "approved" => {}
            "rejected" => {
                return Err(ModuleError::new(
                    ErrorCode::ApprovalDenied,
                    format!(
                        "Approval denied for module '{}': {}",
                        ctx.module_id,
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
                        ctx.module_id,
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
                        ctx.module_id,
                        approval_result
                            .reason
                            .unwrap_or_else(|| "no reason given".to_string())
                    ),
                ));
            }
            _ => {
                tracing::warn!(
                    module_id = %ctx.module_id,
                    status = %approval_result.status,
                    "Unknown approval status, treating as denied"
                );
                return Err(ModuleError::new(
                    ErrorCode::ApprovalDenied,
                    format!(
                        "Approval denied for module '{}': unknown status '{}'",
                        ctx.module_id, approval_result.status
                    ),
                ));
            }
        }

        Ok(StepResult::continue_step())
    }
}

#[async_trait]
impl Step for BuiltinInputValidation {
    impl_step_meta!(BuiltinInputValidation);
    fn requires(&self) -> &[&str] {
        &["module"]
    }
    fn provides(&self) -> &[&str] {
        &["validated_inputs"]
    }

    async fn execute(&self, ctx: &mut PipelineContext) -> Result<StepResult, ModuleError> {
        let module = ctx
            .module
            .as_ref()
            .expect("module must be resolved before input_validation");
        let input_schema = module.input_schema();
        validate_against_schema(&ctx.inputs, &input_schema, "Input")?;

        // Store redacted inputs on context.
        if has_schema(&input_schema) {
            let redacted = redact_sensitive(&ctx.inputs, &input_schema);
            ctx.context.redacted_inputs = Some(
                redacted
                    .as_object()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .collect(),
            );
        }
        ctx.validated_inputs = Some(ctx.inputs.clone());
        Ok(StepResult::continue_step())
    }
}

#[async_trait]
impl Step for BuiltinMiddlewareBefore {
    impl_step_meta!(BuiltinMiddlewareBefore);

    async fn execute(&self, ctx: &mut PipelineContext) -> Result<StepResult, ModuleError> {
        let middleware_manager = match ctx.middleware_manager {
            Some(ref mm) => mm.clone(),
            None => return Ok(StepResult::continue_step()),
        };

        let (modified_inputs, executed) = match middleware_manager
            .execute_before(&ctx.module_id, ctx.inputs.clone(), &ctx.context)
            .await
        {
            Ok((inputs, executed)) => (inputs, executed),
            Err(e) => {
                // On middleware before error, run on_error for recovery.
                let recovery = middleware_manager
                    .execute_on_error(&ctx.module_id, ctx.inputs.clone(), &e, &ctx.context, &[])
                    .await;
                if let Some(recovery_value) = recovery {
                    ctx.output = Some(recovery_value);
                    return Ok(StepResult::skip_to("return_result"));
                }
                return Err(e);
            }
        };
        ctx.inputs = modified_inputs;
        ctx.executed_middlewares = executed;
        Ok(StepResult::continue_step())
    }
}

#[async_trait]
impl Step for BuiltinExecute {
    impl_step_meta!(BuiltinExecute);
    fn requires(&self) -> &[&str] {
        &["module"]
    }
    fn provides(&self) -> &[&str] {
        &["output"]
    }

    async fn execute(&self, ctx: &mut PipelineContext) -> Result<StepResult, ModuleError> {
        let module = ctx
            .module
            .as_ref()
            .expect("module must be resolved before execute")
            .clone();
        let config = ctx
            .config
            .as_ref()
            .expect("config must be injected into PipelineContext");

        // Compute effective timeout: clamp to remaining global deadline (dual-timeout model).
        let mut timeout_ms = config.executor.default_timeout;
        if let Some(deadline) = ctx.context.global_deadline {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64();
            let remaining_ms = ((deadline - now) * 1000.0) as u64;
            if remaining_ms == 0 {
                return Err(ModuleError::new(
                    ErrorCode::ModuleTimeout,
                    format!(
                        "Module '{}' execution aborted: global deadline already exceeded",
                        ctx.module_id
                    ),
                ));
            }
            if timeout_ms == 0 || remaining_ms < timeout_ms {
                timeout_ms = remaining_ms;
            }
        }

        // Streaming path: if ctx.stream is true and module supports streaming,
        // call module.stream() and store chunks, then skip to return_result.
        if ctx.stream && module.supports_stream() {
            let stream_result = module.stream(ctx.inputs.clone(), &ctx.context).await;
            if let Some(result) = stream_result {
                match result {
                    Ok(chunks) => {
                        // Store chunks for the executor's Phase 2/3 processing.
                        ctx.stream_chunks = Some(chunks);
                        return Ok(StepResult::skip_to("return_result"));
                    }
                    Err(e) => {
                        // Stream failed — fall through to error recovery below.
                        if let Some(ref mm) = ctx.middleware_manager {
                            let recovery = mm
                                .execute_on_error(
                                    &ctx.module_id,
                                    ctx.inputs.clone(),
                                    &e,
                                    &ctx.context,
                                    &ctx.executed_middlewares,
                                )
                                .await;
                            if let Some(recovery_value) = recovery {
                                ctx.output = Some(recovery_value);
                                return Ok(StepResult::skip_to("return_result"));
                            }
                        }
                        return Err(e);
                    }
                }
            }
            // module.stream() returned None — fall through to regular execute.
        }

        let execute_result = if timeout_ms > 0 {
            match tokio::time::timeout(
                Duration::from_millis(timeout_ms),
                module.execute(ctx.inputs.clone(), &ctx.context),
            )
            .await
            {
                Ok(result) => result,
                Err(_elapsed) => Err(ModuleError::new(
                    ErrorCode::ModuleTimeout,
                    format!(
                        "Module '{}' execution timed out after {}ms",
                        ctx.module_id, timeout_ms
                    ),
                )),
            }
        } else {
            module.execute(ctx.inputs.clone(), &ctx.context).await
        };

        match execute_result {
            Ok(output) => {
                ctx.output = Some(output);
                Ok(StepResult::continue_step())
            }
            Err(e) => {
                // On execution error, attempt middleware recovery.
                if let Some(ref mm) = ctx.middleware_manager {
                    let recovery = mm
                        .execute_on_error(
                            &ctx.module_id,
                            ctx.inputs.clone(),
                            &e,
                            &ctx.context,
                            &ctx.executed_middlewares,
                        )
                        .await;
                    if let Some(recovery_value) = recovery {
                        ctx.output = Some(recovery_value);
                        return Ok(StepResult::skip_to("return_result"));
                    }
                }
                Err(e)
            }
        }
    }
}

#[async_trait]
impl Step for BuiltinOutputValidation {
    impl_step_meta!(BuiltinOutputValidation);
    fn requires(&self) -> &[&str] {
        &["module", "output"]
    }
    fn provides(&self) -> &[&str] {
        &["validated_output"]
    }

    async fn execute(&self, ctx: &mut PipelineContext) -> Result<StepResult, ModuleError> {
        // In dry_run mode, execute step is skipped so output may be absent.
        let output = match ctx.output.as_ref() {
            Some(o) => o,
            None => return Ok(StepResult::continue_step()),
        };
        let module = ctx
            .module
            .as_ref()
            .expect("module must be resolved before output_validation");
        let output_schema = module.output_schema();
        validate_against_schema(output, &output_schema, "Output")?;

        if has_schema(&output_schema) {
            ctx.context
                .data
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .insert(
                    "_apcore.executor.redacted_output".to_string(),
                    redact_sensitive(output, &output_schema),
                );
        }
        ctx.validated_output = ctx.output.clone();
        Ok(StepResult::continue_step())
    }
}

#[async_trait]
impl Step for BuiltinMiddlewareAfter {
    impl_step_meta!(BuiltinMiddlewareAfter);

    async fn execute(&self, ctx: &mut PipelineContext) -> Result<StepResult, ModuleError> {
        let middleware_manager = match ctx.middleware_manager {
            Some(ref mm) => mm.clone(),
            None => return Ok(StepResult::continue_step()),
        };

        let output = ctx
            .output
            .take()
            .expect("output must be set before middleware_after");

        match middleware_manager
            .execute_after(&ctx.module_id, ctx.inputs.clone(), output, &ctx.context)
            .await
        {
            Ok(modified_output) => {
                ctx.output = Some(modified_output);
                Ok(StepResult::continue_step())
            }
            Err(e) => {
                // On middleware after error, run on_error for recovery.
                let recovery = middleware_manager
                    .execute_on_error(
                        &ctx.module_id,
                        ctx.inputs.clone(),
                        &e,
                        &ctx.context,
                        &ctx.executed_middlewares,
                    )
                    .await;
                if let Some(recovery_value) = recovery {
                    ctx.output = Some(recovery_value);
                    return Ok(StepResult::skip_to("return_result"));
                }
                Err(e)
            }
        }
    }
}

#[async_trait]
impl Step for BuiltinReturnResult {
    impl_step_meta!(BuiltinReturnResult);
    fn requires(&self) -> &[&str] {
        &["output"]
    }

    async fn execute(&self, _ctx: &mut PipelineContext) -> Result<StepResult, ModuleError> {
        // Output is already stored in ctx.output; PipelineEngine returns it.
        Ok(StepResult::continue_step())
    }
}

// ---------------------------------------------------------------------------
// Preset strategies
// ---------------------------------------------------------------------------

/// Build the standard 11-step execution strategy.
pub fn build_standard_strategy() -> ExecutionStrategy {
    // INVARIANT: all step names are unique literals, so `new()` cannot fail.
    ExecutionStrategy::new(
        "standard",
        vec![
            Box::new(BuiltinContextCreation),
            Box::new(BuiltinCallChainGuard),
            Box::new(BuiltinModuleLookup),
            Box::new(BuiltinACLCheck),
            Box::new(BuiltinApprovalGate),
            Box::new(BuiltinMiddlewareBefore),
            Box::new(BuiltinInputValidation),
            Box::new(BuiltinExecute),
            Box::new(BuiltinOutputValidation),
            Box::new(BuiltinMiddlewareAfter),
            Box::new(BuiltinReturnResult),
        ],
    )
    .expect("standard strategy should have unique step names")
}

/// Build the internal strategy (standard minus acl_check and approval_gate).
pub fn build_internal_strategy() -> ExecutionStrategy {
    let mut s = build_standard_strategy();
    s.set_name("internal");
    s.remove("acl_check").ok();
    s.remove("approval_gate").ok();
    s
}

/// Build the testing strategy (standard minus acl_check, approval_gate, and call_chain_guard).
pub fn build_testing_strategy() -> ExecutionStrategy {
    let mut s = build_standard_strategy();
    s.set_name("testing");
    s.remove("acl_check").ok();
    s.remove("approval_gate").ok();
    s.remove("call_chain_guard").ok();
    s
}

/// Build the performance strategy (standard minus middleware_before and middleware_after).
pub fn build_performance_strategy() -> ExecutionStrategy {
    let mut s = build_standard_strategy();
    s.set_name("performance");
    s.remove("middleware_before").ok();
    s.remove("middleware_after").ok();
    s
}

/// Build a minimal strategy: context → lookup → execute → return only.
///
/// No safety checks, no ACL, no approval, no validation, no middleware.
/// Suitable for pre-validated internal hot paths. Use with caution.
pub fn build_minimal_strategy() -> ExecutionStrategy {
    let mut s = build_standard_strategy();
    s.set_name("minimal");
    s.remove("call_chain_guard").ok();
    s.remove("acl_check").ok();
    s.remove("approval_gate").ok();
    s.remove("middleware_before").ok();
    s.remove("input_validation").ok();
    s.remove("output_validation").ok();
    s.remove("middleware_after").ok();
    s
}

// APCore Protocol — Executor
// Spec reference: Module execution engine

use std::time::Duration;

use serde_json::Value;

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
                middleware_manager.add(mw);
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
    pub fn use_middleware(&mut self, middleware: Box<dyn Middleware>) {
        self.middleware_manager.add(middleware);
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
        let child_ctx = parent_ctx.child(module_id);

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
                    let result = handler.request_approval(&request).await?;
                    if result.status != "approved" {
                        return Err(ModuleError::new(
                            ErrorCode::ApprovalDenied,
                            format!(
                                "Approval denied for module '{}': {}",
                                module_id,
                                result
                                    .reason
                                    .unwrap_or_else(|| "no reason given".to_string())
                            ),
                        ));
                    }
                }
            }
        }

        // Step 6: Input Validation — pass through for now

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

        // Step 8: Execute with timeout
        let timeout_ms = self.config.default_timeout_ms;
        let execute_result = if timeout_ms > 0 {
            match tokio::time::timeout(
                Duration::from_millis(timeout_ms),
                module.execute(inputs.clone(), &child_ctx),
            )
            .await
            {
                Ok(result) => result,
                Err(_elapsed) => Err(ModuleError::new(
                    ErrorCode::ModuleTimeout,
                    format!(
                        "Module '{}' execution timed out after {}ms",
                        module_id, timeout_ms
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

        // Step 9: Output Validation — pass through for now

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
                    let result = handler.request_approval(&request).await?;
                    if result.status != "approved" {
                        return Err(ModuleError::new(
                            ErrorCode::ApprovalDenied,
                            format!(
                                "Approval denied for module '{}': {}",
                                module_id,
                                result
                                    .reason
                                    .unwrap_or_else(|| "no reason given".to_string())
                            ),
                        ));
                    }
                }
            }
        }

        // Step 6: Input Validation — pass through for now

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
    pub fn use_before(&mut self, middleware: Box<dyn BeforeMiddleware>) {
        self.middleware_manager
            .add(Box::new(BoxedBeforeMiddlewareAdapter(middleware)));
    }

    /// Add an after middleware.
    pub fn use_after(&mut self, middleware: Box<dyn AfterMiddleware>) {
        self.middleware_manager
            .add(Box::new(BoxedAfterMiddlewareAdapter(middleware)));
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

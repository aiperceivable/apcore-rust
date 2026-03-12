// APCore Protocol — Executor
// Spec reference: Module execution engine

use crate::acl::ACL;
use crate::approval::ApprovalHandler;
use crate::config::Config;
use crate::context::Context;
use crate::errors::ModuleError;
use crate::middleware::manager::MiddlewareManager;
use crate::registry::registry::Registry;

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

    /// Set the ACL for access control.
    pub fn set_acl(&mut self, acl: ACL) {
        self.acl = Some(acl);
    }

    /// Set the approval handler.
    pub fn set_approval_handler(&mut self, handler: Box<dyn ApprovalHandler>) {
        self.approval_handler = Some(handler);
    }

    /// Add a middleware to the pipeline.
    pub fn use_middleware(&mut self, middleware: Box<dyn crate::middleware::base::Middleware>) {
        self.middleware_manager.add(middleware);
    }

    /// Remove a middleware by name.
    pub fn remove_middleware(&mut self, name: &str) -> bool {
        self.middleware_manager.remove(name)
    }

    /// Execute (call) a module by ID with the given inputs and context.
    pub async fn call(
        &self,
        module_id: &str,
        inputs: serde_json::Value,
        ctx: Option<&Context<serde_json::Value>>,
    ) -> Result<serde_json::Value, ModuleError> {
        // TODO: Implement
        todo!()
    }

    /// Validate module inputs without executing.
    pub async fn validate(
        &self,
        module_id: &str,
        inputs: &serde_json::Value,
    ) -> Result<(), ModuleError> {
        // TODO: Implement
        todo!()
    }

    /// Check call depth limits before execution.
    pub fn check_call_depth(&self, ctx: &Context<serde_json::Value>) -> Result<(), ModuleError> {
        // TODO: Implement
        todo!()
    }

    /// Check for circular calls in the call chain.
    pub fn check_circular_call(
        &self,
        ctx: &Context<serde_json::Value>,
        module_id: &str,
    ) -> Result<(), ModuleError> {
        // TODO: Implement
        todo!()
    }
}

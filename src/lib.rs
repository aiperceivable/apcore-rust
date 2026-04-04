// APCore SDK for Rust — AI Partner Core protocol implementation
// Main library module — re-exports all public API
// ModuleError is intentionally large (rich structured error for an SDK); boxing it
// everywhere would change the public API, so we suppress this lint crate-wide.
#![allow(clippy::result_large_err)]

pub mod acl;
pub mod acl_handlers;
pub mod approval;
pub mod async_task;
pub mod bindings;
pub mod builtin_steps;
pub mod cancel;
pub mod client;
pub mod config;
pub mod context;
pub mod context_key;
pub mod context_keys;
pub mod decorator;
pub mod error_formatter;
pub mod errors;
pub mod events;
pub mod executor;
pub mod extensions;
pub mod middleware;
pub mod module;
pub mod observability;
pub mod pipeline;
pub mod registry;
pub mod schema;
pub mod sys_modules;
pub mod trace_context;
pub mod utils;
pub mod version;

// Re-export primary types at crate root for convenience.
pub use acl::{ACLRule, ACL};
pub use acl_handlers::ACLConditionHandler;
pub use approval::{
    AlwaysDenyHandler, ApprovalHandler, ApprovalRequest, ApprovalResult, AutoApproveHandler,
};
pub use async_task::TaskStatus;
pub use builtin_steps::{
    build_internal_strategy, build_performance_strategy, build_standard_strategy,
    build_testing_strategy, BuiltinACLCheck, BuiltinApprovalGate, BuiltinContextCreation,
    BuiltinExecute, BuiltinInputValidation, BuiltinMiddlewareAfter, BuiltinMiddlewareBefore,
    BuiltinModuleLookup, BuiltinOutputValidation, BuiltinReturnResult, BuiltinSafetyCheck,
};
pub use client::APCore;
pub use config::{Config, ConfigMode, EnvStyle, MountSource, NamespaceInfo, NamespaceRegistration};
pub use context::{Context, ContextFactory, Identity};
pub use context_key::ContextKey;
pub use errors::{ErrorCode, ErrorCodeRegistry, ModuleError};
pub use events::emitter::{ApCoreEvent, EventEmitter};
pub use executor::{
    describe_pipeline, list_strategies, redact_sensitive, register_strategy, Executor,
    REDACTED_VALUE,
};
pub use module::{Module, PreflightCheckResult, PreflightResult};
pub use observability::logging::ContextLogger;
pub use observability::tracing_middleware::{SamplingStrategy, TracingMiddleware};
pub use pipeline::{
    ExecutionStrategy, PipelineContext, PipelineEngine, PipelineTrace, Step, StepResult, StepTrace,
    StrategyInfo,
};
pub use registry::registry::Registry;
pub use schema::{ExportProfile, SchemaDefinition, SchemaStrategy};
pub use sys_modules::{
    check_module_disabled, is_module_disabled, register_sys_modules, SysModulesContext, ToggleState,
};

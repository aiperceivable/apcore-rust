// APCore SDK for Rust — AI Partner Core protocol implementation
// Main library module — re-exports all public API
// ModuleError is intentionally large (rich structured error for an SDK); boxing it
// everywhere would change the public API, so we suppress this lint crate-wide.
#![allow(clippy::result_large_err)]

pub mod acl;
pub mod approval;
pub mod async_task;
pub mod bindings;
pub mod cancel;
pub mod client;
pub mod config;
pub mod context;
pub mod decorator;
pub mod error_formatter;
pub mod errors;
pub mod events;
pub mod executor;
pub mod extensions;
pub mod middleware;
pub mod module;
pub mod observability;
pub mod registry;
pub mod schema;
pub mod sys_modules;
pub mod trace_context;
pub mod utils;
pub mod version;

// Re-export primary types at crate root for convenience.
pub use acl::{ACLRule, ACL};
pub use approval::{
    AlwaysDenyHandler, ApprovalHandler, ApprovalRequest, ApprovalResult, AutoApproveHandler,
};
pub use async_task::TaskStatus;
pub use client::APCore;
pub use config::{Config, ConfigMode, MountSource, NamespaceInfo, NamespaceRegistration};
pub use context::{Context, ContextFactory, Identity};
pub use errors::{ErrorCode, ModuleError};
pub use events::emitter::{ApCoreEvent, EventEmitter};
pub use executor::{redact_sensitive, Executor, ValidationResult, REDACTED_VALUE};
pub use module::Module;
pub use observability::logging::ContextLogger;
pub use observability::tracing_middleware::{SamplingStrategy, TracingMiddleware};
pub use registry::registry::Registry;
pub use schema::{ExportProfile, SchemaDefinition, SchemaStrategy};
pub use sys_modules::{
    check_module_disabled, is_module_disabled, register_sys_modules, SysModulesContext, ToggleState,
};

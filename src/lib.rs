// APCore SDK for Rust — AI Partner Core protocol implementation
// Main library module — re-exports all public API
// ModuleError is intentionally large (rich structured error for an SDK); boxing it
// everywhere would change the public API, so we suppress this lint crate-wide.
#![allow(clippy::result_large_err)]

/// The compile-time version of this crate, sourced from Cargo.toml.
///
/// Mirrors `apcore.__version__` in Python and `VERSION` in TypeScript.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

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
pub mod pipeline_config;
pub mod registry;
pub mod schema;
pub mod sys_modules;
pub mod trace_context;
pub mod utils;
pub mod version;

// Re-export primary types at crate root for convenience.
pub use acl::{ACLRule, AuditEntry, ACL};
pub use acl_handlers::ACLConditionHandler;
pub use approval::{
    AlwaysDenyHandler, ApprovalHandler, ApprovalRequest, ApprovalResult, AutoApproveHandler,
    CallbackApprovalHandler,
};
pub use async_task::{AsyncTaskManager, TaskInfo, TaskStatus};
pub use bindings::{BindingDefinition, BindingLoader, BindingTarget};
pub use builtin_steps::{
    build_internal_strategy, build_minimal_strategy, build_performance_strategy,
    build_standard_strategy, build_testing_strategy, BuiltinACLCheck, BuiltinApprovalGate,
    BuiltinCallChainGuard, BuiltinContextCreation, BuiltinExecute, BuiltinInputValidation,
    BuiltinMiddlewareAfter, BuiltinMiddlewareBefore, BuiltinModuleLookup, BuiltinOutputValidation,
    BuiltinReturnResult,
};
pub use cancel::CancelToken;
pub use client::APCore;
pub use config::{
    Config, ConfigMode, EnvStyle, ExecutorConfig, MetricsConfig, MountSource, NamespaceInfo,
    NamespaceRegistration, ObservabilityConfig, TracingConfig,
};
pub use context::{Context, ContextFactory, Identity};
pub use context_key::ContextKey;
pub use context_keys::{
    LOGGING_START, METRICS_STARTS, REDACTED_OUTPUT, RETRY_COUNT_BASE, TRACING_SAMPLED,
    TRACING_SPANS,
};
pub use decorator::FunctionModule;
pub use error_formatter::{ErrorFormatter, ErrorFormatterRegistry};
pub use errors::{
    ErrorCode, ErrorCodeRegistry, ModuleError, VersionIncompatibleError,
    FRAMEWORK_ERROR_CODE_PREFIXES,
};
pub use events::emitter::{ApCoreEvent, EventEmitter};
pub use executor::{
    list_strategies, redact_sensitive, register_strategy, Executor, REDACTED_VALUE,
};
pub use extensions::{ExtensionKind, ExtensionManager, ExtensionPoint};
pub use middleware::{
    AfterMiddleware, BeforeMiddleware, LoggingMiddleware, Middleware, MiddlewareManager,
    PlatformNotifyMiddleware, RetryConfig, RetryMiddleware,
};
pub use module::{
    chunks_to_stream, ChunkStream, Module, ModuleAnnotations, ModuleExample, PreflightCheckResult,
    PreflightResult, ValidationResult, DEFAULT_ANNOTATIONS,
};
pub use observability::error_history::{ErrorEntry, ErrorHistory, ErrorHistoryMiddleware};
pub use observability::exporters::{InMemoryExporter, OTLPExporter, StdoutExporter};
pub use observability::logging::{ContextLogger, ObsLoggingMiddleware};
pub use observability::metrics::{
    MetricsCollector, MetricsMiddleware, METRIC_CALLS_TOTAL, METRIC_DURATION_SECONDS,
};
pub use observability::span::{Span, SpanExporter};
pub use observability::tracing_middleware::{SamplingStrategy, TracingMiddleware};
pub use observability::usage::{UsageCollector, UsageMiddleware, UsageStats};
pub use pipeline::{
    ExecutionStrategy, PipelineContext, PipelineEngine, PipelineTrace, Step, StepResult, StepTrace,
    StrategyInfo,
};
pub use pipeline_config::{
    build_strategy_from_config, register_step_type, registered_step_types, unregister_step_type,
};
pub use registry::registry::{
    module_id_pattern, registry_events, Registry, RegistryEvents, MAX_MODULE_ID_LENGTH,
    MODULE_ID_PATTERN, REGISTRY_EVENTS, RESERVED_WORDS,
};
pub use schema::{
    ExportProfile, RefResolver, SchemaDefinition, SchemaExporter, SchemaLoader, SchemaStrategy,
    SchemaValidator,
};
pub use sys_modules::control::{ReloadModuleModule, ToggleFeatureModule, UpdateConfigModule};
pub use sys_modules::health::{HealthModuleModule, HealthSummaryModule};
pub use sys_modules::manifest::{ManifestFullModule, ManifestModuleModule};
pub use sys_modules::usage::{UsageModuleModule, UsageSummaryModule};
pub use sys_modules::{
    check_module_disabled, is_module_disabled, register_sys_modules, SysModulesContext, ToggleState,
};
pub use trace_context::{TraceContext, TraceParent};
pub use utils::{
    calculate_specificity, guard_call_chain, guard_call_chain_with_repeat, match_pattern,
    normalize_to_canonical_id, propagate_error, propagate_module_error, DEFAULT_MAX_CALL_DEPTH,
    DEFAULT_MAX_MODULE_REPEAT,
};
pub use version::negotiate_version;

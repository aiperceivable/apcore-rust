// APCore SDK for Rust — AI Partner Core protocol implementation
// Main library module — re-exports all public API
// ModuleError is intentionally large (rich structured error for an SDK); boxing it
// everywhere would change the public API, so we suppress this lint crate-wide.
#![allow(clippy::result_large_err)]
// ACL, ACL-rule, etc. are protocol-defined uppercase acronyms — matches Python/TS naming.
#![allow(clippy::upper_case_acronyms)]

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
// Note: `async_task::RetryConfig` is intentionally NOT re-exported at the
// crate root because the name collides with `middleware::RetryConfig`. Users
// MUST import it via `apcore::async_task::RetryConfig`.
pub use async_task::{
    AsyncTaskManager, InMemoryTaskStore, ReaperConfig, ReaperHandle, TaskInfo, TaskStatus,
    TaskStore,
};
pub use bindings::{
    typed_handler, AutoSchemaValue, BindingEntry, BindingHandler, BindingLoader, BindingsFile,
    TypedBindingHandler,
};
pub use builtin_steps::{
    build_internal_strategy, build_minimal_strategy, build_performance_strategy,
    build_standard_strategy, build_testing_strategy, BuiltinACLCheck, BuiltinApprovalGate,
    BuiltinCallChainGuard, BuiltinContextCreation, BuiltinExecute, BuiltinInputValidation,
    BuiltinMiddlewareAfter, BuiltinMiddlewareBefore, BuiltinModuleLookup, BuiltinOutputValidation,
    BuiltinReturnResult,
};
pub use cancel::{CancelToken, ExecutionCancelledError};
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
pub use events::circuit_breaker::{
    CircuitBreakerWrapper, CircuitEventSink, CircuitState, DEFAULT_OPEN_THRESHOLD,
    DEFAULT_RECOVERY_WINDOW_MS, DEFAULT_TIMEOUT_MS,
};
pub use events::emitter::{ApCoreEvent, EventEmitter};
pub use events::subscribers::{
    register_subscriber_type, reset_subscriber_registry, unregister_subscriber_type, A2ASubscriber,
    EventSubscriber, FileSubscriber, FilterSubscriber, OutputFormat, StdoutSubscriber,
    WebhookSubscriber,
};
pub use executor::{
    list_strategies, redact_sensitive, register_strategy, Executor, REDACTED_VALUE,
};
pub use extensions::{ExtensionKind, ExtensionManager, ExtensionPoint};
pub use middleware::{
    AfterMiddleware, BeforeMiddleware, CircuitBreakerBuilder, CircuitBreakerConfig,
    CircuitBreakerMiddleware, CircuitBreakerMiddlewareConfig, CircuitBreakerState, ContextWriter,
    CustomMiddlewareConfig, CustomMiddlewareFactory, LoggingMiddleware, LoggingMiddlewareConfig,
    Middleware, MiddlewareChainConfig, MiddlewareConfig, MiddlewareFactory, MiddlewareManager,
    NamespaceCheck, OnErrorOutcome, OtelTracingBuilder, OtelTracingConfig, OtelTracingMiddleware,
    PlatformNotifyMiddleware, RetryConfig, RetryMiddleware, RetrySignal, TracingMiddlewareConfig,
    APCORE_KEY_PREFIX, EXT_KEY_PREFIX,
};
pub use module::{
    ChunkStream, Module, ModuleAnnotations, ModuleExample, PreflightCheckResult, PreflightResult,
    ValidationResult, DEFAULT_ANNOTATIONS,
};
pub use observability::error_history::{
    compute_fingerprint, normalize_message, ErrorEntry, ErrorHistory, ErrorHistoryMiddleware,
};
pub use observability::exporters::{InMemoryExporter, OTLPExporter, StdoutExporter};
pub use observability::logging::{ContextLogger, ObsLoggingMiddleware};
pub use observability::metrics::{
    MetricsCollector, MetricsMiddleware, METRIC_CALLS_TOTAL, METRIC_DURATION_SECONDS,
};
pub use observability::processor::{
    BatchSpanProcessor, BatchSpanProcessorBuilder, BatchSpanProcessorConfig, SimpleSpanProcessor,
    SpanProcessor,
};
pub use observability::prometheus_exporter::PrometheusExporter;
pub use observability::redaction::{RedactionConfig, RedactionConfigBuilder, RedactionConfigError};
pub use observability::span::{Span, SpanExporter};
pub use observability::store::{InMemoryObservabilityStore, MetricPoint, ObservabilityStore};
pub use observability::tracing_middleware::{SamplingStrategy, TracingMiddleware};
pub use observability::usage::{UsageCollector, UsageMiddleware, UsageStats};
pub use pipeline::{
    ExecutionStrategy, PipelineContext, PipelineEngine, PipelineState, PipelineTrace, RunOptions,
    RunUntilPredicate, Step, StepResult, StepTrace, StrategyInfo,
};
pub use pipeline_config::{
    build_strategy_from_config, register_step_type, registered_step_types, unregister_step_type,
};
pub use registry::registry::{
    module_id_pattern, registry_events, Registry, RegistryEvents, DEFAULT_MODULE_VERSION,
    MAX_MODULE_ID_LENGTH, MODULE_ID_PATTERN, REGISTRY_EVENTS, RESERVED_WORDS,
};
pub use registry::{
    class_name_to_segment, compute_base_id, derive_module_ids, detect_id_conflicts, ConflictResult,
    ConflictSeverity, ConflictType, DefaultDiscoverer, DiscoveredClass, DiscoveryConfig,
    ModuleFactory, MultiClassEntry, MAX_MODULE_ID_LEN,
};
pub use schema::{
    to_strict_schema, ExportProfile, RefResolver, SchemaDefinition, SchemaExporter, SchemaLoader,
    SchemaStrategy, SchemaValidator,
};
pub use sys_modules::audit::{
    AuditAction as SysAuditAction, AuditChange as SysAuditChange, AuditEntry as SysAuditEntry,
    AuditStore as SysAuditStore, InMemoryAuditStore as SysInMemoryAuditStore,
};
pub use sys_modules::control::UpdateConfigModule;
pub use sys_modules::{
    check_module_disabled, is_module_disabled, register_sys_modules,
    register_sys_modules_with_options, SysModuleError, SysModulesContext, SysModulesOptions,
    ToggleState,
};
pub use trace_context::{TraceContext, TraceParent};
pub use utils::{
    calculate_specificity, guard_call_chain, guard_call_chain_with_repeat, match_pattern,
    normalize_to_canonical_id, propagate_error, propagate_module_error, DEFAULT_MAX_CALL_DEPTH,
    DEFAULT_MAX_MODULE_REPEAT,
};
pub use version::negotiate_version;

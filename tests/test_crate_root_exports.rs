//! Crate-root re-export parity test.
//!
//! Verifies that the symbols required by PROTOCOL_SPEC §12.2 (Core Component
//! Interface Contracts) and §8.8 (ErrorFormatterRegistry), plus the symbols
//! that `apcore-python` and `apcore-typescript` expose at their package root,
//! are all accessible directly from `apcore::*` without requiring callers to
//! navigate the internal module path.
//!
//! Regression for sync findings A-003, A-004, A-005, A-007, A-008 from
//! 2026-04-08, and D1-004, D1-005 from 2026-04-10.
//! Adding a new spec-required symbol? Add a row here too.

#![allow(unused_imports, dead_code)]

#[test]
fn test_middleware_manager_at_crate_root() {
    // A-003: PROTOCOL_SPEC §12.2 — MiddlewareManager is a required core component.
    use apcore::{Middleware, MiddlewareManager};
    let _: fn() -> MiddlewareManager = MiddlewareManager::new;
}

#[test]
fn test_middleware_concrete_classes_at_crate_root() {
    // Parity with apcore-python / apcore-typescript built-in middleware exports.
    use apcore::{
        AfterMiddleware, BeforeMiddleware, ErrorHistoryMiddleware, LoggingMiddleware,
        MetricsMiddleware, ObsLoggingMiddleware, PlatformNotifyMiddleware, RetryMiddleware,
        UsageMiddleware,
    };
    // Compile-time only — referencing the type ensures it resolves at the crate root.
    let _: Option<RetryMiddleware> = None;
    let _: Option<LoggingMiddleware> = None;
    let _: Option<MetricsMiddleware> = None;
    let _: Option<UsageMiddleware> = None;
    let _: Option<ErrorHistoryMiddleware> = None;
    let _: Option<ObsLoggingMiddleware> = None;
}

#[test]
fn test_error_formatter_registry_at_crate_root() {
    // A-004: PROTOCOL_SPEC §8.8 — ErrorFormatterRegistry is normative.
    use apcore::{ErrorFormatter, ErrorFormatterRegistry};
    // Calling a static method confirms the type and its inherent impl are reachable.
    let _ = ErrorFormatterRegistry::is_registered("nonexistent");
}

#[test]
fn test_build_minimal_strategy_at_crate_root() {
    // A-005: parity with apcore-typescript's `buildMinimalStrategy` export.
    use apcore::{
        build_internal_strategy, build_minimal_strategy, build_performance_strategy,
        build_standard_strategy, build_testing_strategy,
    };
    let _strategy = build_minimal_strategy();
}

#[test]
fn test_module_id_pattern_at_crate_root() {
    // A-007: parity with Python `MODULE_ID_PATTERN` and TypeScript
    // `MODULE_ID_PATTERN` constants — exposed in Rust as a function returning
    // `&'static Regex` due to lazy initialization, but reachable from the crate root.
    use apcore::module_id_pattern;
    let pattern = module_id_pattern();
    assert!(pattern.is_match("foo.bar"));
    assert!(pattern.is_match("a"));
    assert!(!pattern.is_match("Foo.bar"));
    assert!(!pattern.is_match("foo-bar"));
}

#[test]
fn test_registry_events_constants_at_crate_root() {
    // A-008: PROTOCOL_SPEC §12.2 MUST — "All SDKs MUST export these event
    // names as named constants. Consumers MUST NOT hardcode event name strings."
    use apcore::{registry_events, RegistryEvents, REGISTRY_EVENTS};

    // Module-style access: apcore::registry_events::REGISTER
    assert_eq!(registry_events::REGISTER, "register");
    assert_eq!(registry_events::UNREGISTER, "unregister");

    // Struct-associated-const access: apcore::RegistryEvents::REGISTER
    assert_eq!(RegistryEvents::REGISTER, "register");
    assert_eq!(RegistryEvents::UNREGISTER, "unregister");

    // Singleton instance access (parity with Python `REGISTRY_EVENTS["REGISTER"]`
    // and TypeScript `REGISTRY_EVENTS.REGISTER`): apcore::REGISTRY_EVENTS::REGISTER
    let _ = REGISTRY_EVENTS;
    assert_eq!(RegistryEvents::REGISTER, registry_events::REGISTER);
}

#[test]
fn test_pipeline_engine_at_crate_root() {
    // D1-004: parity with apcore-typescript's `PipelineEngine` export.
    use apcore::{
        ExecutionStrategy, PipelineContext, PipelineEngine, PipelineTrace, Step, StepResult,
        StepTrace, StrategyInfo,
    };
    // Compile-time — confirms the type resolves at the crate root.
    let _: Option<PipelineTrace> = None;
    let _: Option<StepTrace> = None;
    let _: Option<StrategyInfo> = None;
}

#[test]
fn test_builtin_pipeline_steps_at_crate_root() {
    // D1-005: parity with apcore-typescript's 11 Builtin* step exports.
    use apcore::{
        BuiltinACLCheck, BuiltinApprovalGate, BuiltinCallChainGuard, BuiltinContextCreation,
        BuiltinExecute, BuiltinInputValidation, BuiltinMiddlewareAfter, BuiltinMiddlewareBefore,
        BuiltinModuleLookup, BuiltinOutputValidation, BuiltinReturnResult,
    };
    // Compile-time references — if any aren't at the crate root, this test fails to compile.
    let _ = BuiltinContextCreation;
    let _ = BuiltinCallChainGuard;
    let _ = BuiltinModuleLookup;
    let _ = BuiltinACLCheck;
    let _ = BuiltinApprovalGate;
    let _ = BuiltinInputValidation;
    let _ = BuiltinMiddlewareBefore;
    let _ = BuiltinExecute;
    let _ = BuiltinOutputValidation;
    let _ = BuiltinMiddlewareAfter;
    let _ = BuiltinReturnResult;
}

#[test]
fn test_other_required_exports_at_crate_root() {
    // Parity sweep — every symbol Python/TypeScript expose at the package root
    // should also be reachable from `apcore::*`.
    use apcore::{
        // Bindings
        BindingDefinition,
        BindingLoader,
        BindingTarget,
        // Cancel
        CancelToken,
        // Observability extras
        ErrorEntry,
        ErrorHistory,
        // Decorator
        FunctionModule,
        InMemoryExporter,
        MetricsCollector,
        OTLPExporter,
        // Schema
        RefResolver,
        SchemaExporter,
        SchemaLoader,
        SchemaValidator,
        Span,
        SpanExporter,
        StdoutExporter,
        // Tracing
        TraceContext,
        TraceParent,
        UsageCollector,
        UsageStats,
    };
    // Compile-time references — if any of these aren't at the crate root, this test fails to compile.
    let _: Option<CancelToken> = None;
}

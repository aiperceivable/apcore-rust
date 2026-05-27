//! Tests for ModuleError and ErrorCode.

use apcore::errors::{ErrorCode, ModuleError};

// ---------------------------------------------------------------------------
// ErrorCode
// ---------------------------------------------------------------------------

#[test]
fn test_error_code_equality() {
    assert_eq!(ErrorCode::ModuleNotFound, ErrorCode::ModuleNotFound);
    assert_ne!(ErrorCode::ModuleNotFound, ErrorCode::ACLDenied);
}

#[test]
fn test_error_code_serialization() {
    let code = ErrorCode::SchemaValidationError;
    let json = serde_json::to_string(&code).unwrap();
    assert_eq!(json, "\"SCHEMA_VALIDATION_ERROR\"");
}

#[test]
fn test_error_code_deserialization() {
    let code: ErrorCode = serde_json::from_str("\"ACL_DENIED\"").unwrap();
    assert_eq!(code, ErrorCode::ACLDenied);
}

/// Regression for sync ERR-001: CircuitBreakerOpen must serialize as
/// `CIRCUIT_BREAKER_OPEN` to align with apcore-python and apcore-typescript.
#[test]
fn test_circuit_breaker_open_serde_rename_aligns_cross_language() {
    let json = serde_json::to_value(ErrorCode::CircuitBreakerOpen).unwrap();
    assert_eq!(json, serde_json::json!("CIRCUIT_BREAKER_OPEN"));
    let parsed: ErrorCode = serde_json::from_str("\"CIRCUIT_BREAKER_OPEN\"").unwrap();
    assert_eq!(parsed, ErrorCode::CircuitBreakerOpen);
}

#[test]
fn test_all_error_codes_defined() {
    // Verify the full set matches the protocol spec (37 codes).
    let codes = [
        ErrorCode::ConfigNotFound,
        ErrorCode::ConfigInvalid,
        ErrorCode::ACLRuleError,
        ErrorCode::ACLDenied,
        ErrorCode::ModuleNotFound,
        ErrorCode::ModuleDisabled,
        ErrorCode::ModuleTimeout,
        ErrorCode::ModuleLoadError,
        ErrorCode::ModuleExecuteError,
        ErrorCode::ReloadFailed,
        ErrorCode::ExecutionCancelled,
        ErrorCode::SchemaValidationError,
        ErrorCode::SchemaNotFound,
        ErrorCode::SchemaParseError,
        ErrorCode::SchemaCircularRef,
        ErrorCode::CallDepthExceeded,
        ErrorCode::CircularCall,
        ErrorCode::CallFrequencyExceeded,
        ErrorCode::GeneralInvalidInput,
        ErrorCode::GeneralInternalError,
        ErrorCode::GeneralNotImplemented,
        ErrorCode::FuncMissingTypeHint,
        ErrorCode::FuncMissingReturnType,
        ErrorCode::BindingInvalidTarget,
        ErrorCode::BindingModuleNotFound,
        ErrorCode::BindingCallableNotFound,
        ErrorCode::BindingNotCallable,
        ErrorCode::BindingSchemaMissing,
        ErrorCode::BindingFileInvalid,
        ErrorCode::CircularDependency,
        ErrorCode::MiddlewareChainError,
        ErrorCode::ApprovalDenied,
        ErrorCode::ApprovalTimeout,
        ErrorCode::ApprovalPending,
        ErrorCode::VersionIncompatible,
        ErrorCode::ErrorCodeCollision,
        ErrorCode::DependencyNotFound,
    ];
    assert_eq!(codes.len(), 37, "Protocol defines exactly 37 error codes");
}

// ---------------------------------------------------------------------------
// ModuleError
// ---------------------------------------------------------------------------

#[test]
fn test_module_error_basic_fields() {
    let err = ModuleError::new(ErrorCode::ModuleNotFound, "module 'foo' not found");
    assert_eq!(err.code, ErrorCode::ModuleNotFound);
    assert_eq!(err.message, "module 'foo' not found");
}

#[test]
fn test_module_error_not_retryable_by_default() {
    let err = ModuleError::new(ErrorCode::GeneralInternalError, "oops");
    assert_eq!(err.retryable, None);
}

#[test]
fn test_module_error_not_user_fixable_by_default() {
    let err = ModuleError::new(ErrorCode::GeneralInternalError, "oops");
    assert_eq!(err.user_fixable, None);
}

#[test]
fn test_module_error_no_cause_by_default() {
    let err = ModuleError::new(ErrorCode::GeneralInternalError, "oops");
    assert!(err.cause.is_none());
}

#[test]
fn test_module_error_no_trace_id_by_default() {
    let err = ModuleError::new(ErrorCode::GeneralInternalError, "oops");
    assert!(err.trace_id.is_none());
}

#[test]
fn test_module_error_display() {
    let err = ModuleError::new(ErrorCode::ACLDenied, "access denied");
    let s = format!("{err}");
    assert!(s.contains("ACLDenied"));
    assert!(s.contains("access denied"));
}

#[test]
fn test_module_error_details_empty_by_default() {
    let err = ModuleError::new(ErrorCode::GeneralInvalidInput, "bad input");
    assert!(err.details.is_empty());
}

#[test]
fn test_module_error_with_details() {
    let mut err = ModuleError::new(ErrorCode::SchemaValidationError, "field missing");
    err.details
        .insert("field".to_string(), serde_json::json!("user_id"));
    assert_eq!(err.details["field"], "user_id");
}

#[test]
fn test_module_error_serialization_round_trip() {
    let err = ModuleError::new(ErrorCode::ModuleTimeout, "timed out after 30s");
    let json = serde_json::to_string(&err).unwrap();
    let restored: ModuleError = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.code, ErrorCode::ModuleTimeout);
    assert_eq!(restored.message, "timed out after 30s");
}

#[test]
fn test_module_error_is_std_error() {
    let err = ModuleError::new(ErrorCode::GeneralInternalError, "oops");
    // Verify it satisfies std::error::Error
    let _: &dyn std::error::Error = &err;
}

// ---------------------------------------------------------------------------
// A-D-015: MiddlewareChainError unwrap recovers the original typed error
// ---------------------------------------------------------------------------

#[test]
fn unwrap_middleware_chain_error_recovers_inner() {
    let inner = ModuleError::new(ErrorCode::ApprovalDenied, "approval rejected");
    let mut details = std::collections::HashMap::new();
    details.insert(
        "inner_error".to_string(),
        serde_json::to_value(&inner).unwrap(),
    );
    let wrapped = ModuleError::new(ErrorCode::MiddlewareChainError, inner.message.clone())
        .with_details(details);

    let recovered = wrapped.unwrap_middleware_chain_error().expect("unwrap");
    assert_eq!(recovered.code, ErrorCode::ApprovalDenied);
    assert_eq!(recovered.message, "approval rejected");
}

#[test]
fn unwrap_middleware_chain_error_returns_none_for_other_codes() {
    let err = ModuleError::new(ErrorCode::ModuleTimeout, "timeout");
    assert!(err.unwrap_middleware_chain_error().is_none());
}

#[test]
fn unwrap_middleware_chain_error_returns_none_when_inner_missing() {
    let err = ModuleError::new(ErrorCode::MiddlewareChainError, "no inner");
    assert!(err.unwrap_middleware_chain_error().is_none());
}

// ---------------------------------------------------------------------------
// A-D-006 + A-D-007: 14 canonical reserved prefixes + exact framework-code
// collision check.
// ---------------------------------------------------------------------------

use apcore::errors::{ErrorCodeRegistry, FRAMEWORK_ERROR_CODE_PREFIXES};
use std::collections::HashSet;

fn code_set(codes: &[&str]) -> HashSet<String> {
    codes.iter().map(|s| (*s).to_string()).collect()
}

/// A-D-006: the reserved-prefix set is exactly the canonical 14, with the
/// four non-canonical prefixes (CIRCUIT_, PIPELINE_, STEP_, STRATEGY_) dropped.
#[test]
fn test_reserved_prefixes_are_canonical_14() {
    let expected: HashSet<&str> = [
        "ACL_",
        "APPROVAL_",
        "BINDING_",
        "CALL_",
        "CIRCULAR_",
        "CONFIG_",
        "DEPENDENCY_",
        "ERROR_CODE_",
        "FUNC_",
        "GENERAL_",
        "MIDDLEWARE_",
        "MODULE_",
        "SCHEMA_",
        "VERSION_",
    ]
    .into_iter()
    .collect();
    let actual: HashSet<&str> = FRAMEWORK_ERROR_CODE_PREFIXES.iter().copied().collect();
    assert_eq!(actual, expected);
    assert_eq!(FRAMEWORK_ERROR_CODE_PREFIXES.len(), 14);
    for dropped in ["CIRCUIT_", "PIPELINE_", "STEP_", "STRATEGY_"] {
        assert!(!actual.contains(dropped), "{dropped} must not be reserved");
    }
}

/// A-D-006: a custom code with a `STEP_` prefix that is NOT a framework code
/// now registers successfully (prefix no longer reserved).
#[test]
fn test_register_step_custom_succeeds_after_prefix_narrowing() {
    let mut reg = ErrorCodeRegistry::new();
    let result = reg.register("m", &code_set(&["STEP_CUSTOM"]));
    assert!(result.is_ok(), "STEP_CUSTOM should register: {result:?}");
}

/// A-D-007: an exact framework code that no longer matches any prefix
/// (`STEP_NOT_FOUND`) is still rejected via the exact-code check.
#[test]
fn test_register_step_not_found_rejected_as_framework_code() {
    let mut reg = ErrorCodeRegistry::new();
    let err = reg
        .register("m", &code_set(&["STEP_NOT_FOUND"]))
        .expect_err("STEP_NOT_FOUND is a framework code");
    assert_eq!(err.code, ErrorCode::ErrorCodeCollision);
    assert_eq!(
        err.details.get("conflict_source").and_then(|v| v.as_str()),
        Some("framework")
    );
}

/// A-D-007: a framework code with no reserved prefix at all (`RELOAD_FAILED`)
/// is rejected by the exact-code check.
#[test]
fn test_register_reload_failed_rejected_as_framework_code() {
    let mut reg = ErrorCodeRegistry::new();
    let err = reg
        .register("m", &code_set(&["RELOAD_FAILED"]))
        .expect_err("RELOAD_FAILED is a framework code");
    assert_eq!(err.code, ErrorCode::ErrorCodeCollision);
    assert_eq!(
        err.details.get("conflict_source").and_then(|v| v.as_str()),
        Some("framework")
    );
}

/// A-D-007: CIRCUIT_BREAKER_OPEN / PIPELINE_STEP_ERROR / STRATEGY_NOT_FOUND
/// remain protected by the exact-code check after prefix narrowing.
#[test]
fn test_register_non_prefix_framework_codes_rejected() {
    for code in [
        "CIRCUIT_BREAKER_OPEN",
        "PIPELINE_STEP_ERROR",
        "STRATEGY_NOT_FOUND",
    ] {
        let mut reg = ErrorCodeRegistry::new();
        assert!(
            reg.register("m", &code_set(&[code])).is_err(),
            "{code} must be rejected as framework code"
        );
    }
}

/// A-D-021: a fresh registry (no modules) seeds `all_codes` with the framework
/// code set, so it is non-empty and contains framework codes.
#[test]
fn test_fresh_registry_all_codes_contains_framework_codes() {
    let reg = ErrorCodeRegistry::new();
    assert!(!reg.all_codes().is_empty());
    assert!(reg.all_codes().contains("SCHEMA_VALIDATION_ERROR"));
    assert!(reg.all_codes().contains("CIRCUIT_BREAKER_OPEN"));
    assert!(reg.all_codes().contains("RELOAD_FAILED"));
}

/// A-D-021: after registering module codes, `all_codes` still contains the
/// framework codes (rebuild must not drop them).
#[test]
fn test_all_codes_retains_framework_after_module_register() {
    let mut reg = ErrorCodeRegistry::new();
    reg.register("m", &code_set(&["MY_CUSTOM_CODE"])).unwrap();
    assert!(reg.all_codes().contains("MY_CUSTOM_CODE"));
    assert!(reg.all_codes().contains("SCHEMA_VALIDATION_ERROR"));
}

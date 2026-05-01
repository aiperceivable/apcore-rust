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

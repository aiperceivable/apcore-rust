// APCore SDK — Integration tests
// Basic smoke tests to verify the skeleton compiles and types are accessible.

use apcore::cancel::CancelToken;
use apcore::context::Identity;
use apcore::errors::ErrorCode;

#[test]
fn test_error_code_variants_exist() {
    // Verify all critical ErrorCode variants are defined.
    let codes = vec![
        ErrorCode::ConfigNotFound,
        ErrorCode::ConfigInvalid,
        ErrorCode::AclRuleError,
        ErrorCode::AclDenied,
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
    assert_eq!(codes.len(), 37);
}

#[test]
fn test_cancel_token() {
    let token = CancelToken::new();
    assert!(!token.is_cancelled());
    token.cancel();
    assert!(token.is_cancelled());
}

#[test]
fn test_identity_creation() {
    let identity = Identity {
        id: "user-1".to_string(),
        identity_type: "Test User".to_string(),
        roles: vec!["admin".to_string()],
        attrs: std::collections::HashMap::new(),
    };
    assert_eq!(identity.id, "user-1");
    assert_eq!(identity.roles.len(), 1);
}

#[test]
fn test_module_error_creation() {
    let error =
        apcore::errors::ModuleError::new(ErrorCode::GeneralInternalError, "something went wrong");
    assert_eq!(error.code, ErrorCode::GeneralInternalError);
    assert!(!error.retryable);
    assert!(!error.user_fixable);
}

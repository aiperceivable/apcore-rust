// APCore Protocol — Error types
// Spec reference: Error handling and ErrorCode enumeration

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

/// All error codes defined by the APCore protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    ConfigNotFound,
    ConfigInvalid,
    AclRuleError,
    AclDenied,
    ModuleNotFound,
    ModuleDisabled,
    ModuleTimeout,
    ModuleLoadError,
    ModuleExecuteError,
    ReloadFailed,
    ExecutionCancelled,
    SchemaValidationError,
    SchemaNotFound,
    SchemaParseError,
    SchemaCircularRef,
    CallDepthExceeded,
    CircularCall,
    CallFrequencyExceeded,
    GeneralInvalidInput,
    GeneralInternalError,
    GeneralNotImplemented,
    FuncMissingTypeHint,
    FuncMissingReturnType,
    BindingInvalidTarget,
    BindingModuleNotFound,
    BindingCallableNotFound,
    BindingNotCallable,
    BindingSchemaMissing,
    BindingFileInvalid,
    CircularDependency,
    MiddlewareChainError,
    ApprovalDenied,
    ApprovalTimeout,
    ApprovalPending,
    VersionIncompatible,
    ErrorCodeCollision,
    DependencyNotFound,
}

/// Structured error returned by module execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default)]
    pub details: HashMap<String, serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    pub timestamp: DateTime<Utc>,
    #[serde(default)]
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ai_guidance: Option<String>,
    #[serde(default)]
    pub user_fixable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
}

impl ModuleError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: HashMap::new(),
            cause: None,
            trace_id: None,
            timestamp: Utc::now(),
            retryable: false,
            ai_guidance: None,
            user_fixable: false,
            suggestion: None,
        }
    }
}

impl std::fmt::Display for ModuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{:?}] {}", self.code, self.message)
    }
}

impl std::error::Error for ModuleError {}

// Named error types using thiserror

#[derive(Debug, Error)]
#[error("Config not found: {message}")]
pub struct ConfigNotFoundError {
    pub message: String,
}

#[derive(Debug, Error)]
#[error("Config invalid: {message}")]
pub struct ConfigInvalidError {
    pub message: String,
}

#[derive(Debug, Error)]
#[error("ACL rule error: {message}")]
pub struct AclRuleError {
    pub message: String,
}

#[derive(Debug, Error)]
#[error("ACL denied: {message}")]
pub struct AclDeniedError {
    pub message: String,
}

#[derive(Debug, Error)]
#[error("Module not found: {message}")]
pub struct ModuleNotFoundError {
    pub message: String,
}

#[derive(Debug, Error)]
#[error("Module disabled: {message}")]
pub struct ModuleDisabledError {
    pub message: String,
}

#[derive(Debug, Error)]
#[error("Module timeout: {message}")]
pub struct ModuleTimeoutError {
    pub message: String,
}

#[derive(Debug, Error)]
#[error("Module load error: {message}")]
pub struct ModuleLoadError {
    pub message: String,
}

#[derive(Debug, Error)]
#[error("Module execute error: {message}")]
pub struct ModuleExecuteError {
    pub message: String,
}

#[derive(Debug, Error)]
#[error("Reload failed: {message}")]
pub struct ReloadFailedError {
    pub message: String,
}

#[derive(Debug, Error)]
#[error("Execution cancelled: {message}")]
pub struct ExecutionCancelledError {
    pub message: String,
}

#[derive(Debug, Error)]
#[error("Schema validation error: {message}")]
pub struct SchemaValidationError {
    pub message: String,
}

#[derive(Debug, Error)]
#[error("Schema not found: {message}")]
pub struct SchemaNotFoundError {
    pub message: String,
}

#[derive(Debug, Error)]
#[error("Schema parse error: {message}")]
pub struct SchemaParseError {
    pub message: String,
}

#[derive(Debug, Error)]
#[error("Schema circular ref: {message}")]
pub struct SchemaCircularRefError {
    pub message: String,
}

#[derive(Debug, Error)]
#[error("Call depth exceeded: {message}")]
pub struct CallDepthExceededError {
    pub message: String,
}

#[derive(Debug, Error)]
#[error("Circular call detected: {message}")]
pub struct CircularCallError {
    pub message: String,
}

#[derive(Debug, Error)]
#[error("Call frequency exceeded: {message}")]
pub struct CallFrequencyExceededError {
    pub message: String,
}

#[derive(Debug, Error)]
#[error("Invalid input: {message}")]
pub struct InvalidInputError {
    pub message: String,
}

#[derive(Debug, Error)]
#[error("Internal error: {message}")]
pub struct InternalError {
    pub message: String,
}

#[derive(Debug, Error)]
#[error("Not implemented: {message}")]
pub struct NotImplementedError {
    pub message: String,
}

#[derive(Debug, Error)]
#[error("Missing type hint: {message}")]
pub struct FuncMissingTypeHintError {
    pub message: String,
}

#[derive(Debug, Error)]
#[error("Missing return type: {message}")]
pub struct FuncMissingReturnTypeError {
    pub message: String,
}

#[derive(Debug, Error)]
#[error("Binding invalid target: {message}")]
pub struct BindingInvalidTargetError {
    pub message: String,
}

#[derive(Debug, Error)]
#[error("Binding module not found: {message}")]
pub struct BindingModuleNotFoundError {
    pub message: String,
}

#[derive(Debug, Error)]
#[error("Binding callable not found: {message}")]
pub struct BindingCallableNotFoundError {
    pub message: String,
}

#[derive(Debug, Error)]
#[error("Binding not callable: {message}")]
pub struct BindingNotCallableError {
    pub message: String,
}

#[derive(Debug, Error)]
#[error("Binding schema missing: {message}")]
pub struct BindingSchemaMissingError {
    pub message: String,
}

#[derive(Debug, Error)]
#[error("Binding file invalid: {message}")]
pub struct BindingFileInvalidError {
    pub message: String,
}

#[derive(Debug, Error)]
#[error("Circular dependency: {message}")]
pub struct CircularDependencyError {
    pub message: String,
}

#[derive(Debug, Error)]
#[error("Middleware chain error: {message}")]
pub struct MiddlewareChainError {
    pub message: String,
}

#[derive(Debug, Error)]
#[error("Approval denied: {message}")]
pub struct ApprovalDeniedError {
    pub message: String,
}

#[derive(Debug, Error)]
#[error("Approval timeout: {message}")]
pub struct ApprovalTimeoutError {
    pub message: String,
}

#[derive(Debug, Error)]
#[error("Approval pending: {message}")]
pub struct ApprovalPendingError {
    pub message: String,
}

#[derive(Debug, Error)]
#[error("Version incompatible: {message}")]
pub struct VersionIncompatibleError {
    pub message: String,
}

#[derive(Debug, Error)]
#[error("Error code collision: {message}")]
pub struct ErrorCodeCollisionError {
    pub message: String,
}

#[derive(Debug, Error)]
#[error("Dependency not found: {message}")]
pub struct DependencyNotFoundError {
    pub message: String,
}

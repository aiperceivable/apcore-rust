// APCore Protocol — Error types
// Spec reference: Error handling and ErrorCode enumeration
// Aligned to apcore-python reference implementation

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use thiserror::Error;

/// Framework error code prefixes reserved by the protocol.
pub const FRAMEWORK_ERROR_CODE_PREFIXES: &[&str] = &[
    "CONFIG_",
    "ACL_",
    "MODULE_",
    "SCHEMA_",
    "CALL_",
    "CIRCULAR_",
    "GENERAL_",
    "FUNC_",
    "BINDING_",
    "MIDDLEWARE_",
    "APPROVAL_",
    "VERSION_",
    "ERROR_CODE_",
    "DEPENDENCY_",
    "EXECUTION_",
    "RELOAD_",
];

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
    ConfigNamespaceDuplicate,
    ConfigNamespaceReserved,
    ConfigEnvPrefixConflict,
    ConfigMountError,
    ConfigBindError,
    ConfigEnvMapConflict,
    ErrorFormatterDuplicate,
}

/// Structured error returned by module execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub details: HashMap<String, serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    pub timestamp: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ai_guidance: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_fixable: Option<bool>,
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
            retryable: None,
            ai_guidance: None,
            user_fixable: None,
            suggestion: None,
        }
    }

    // Builder methods

    pub fn with_details(mut self, details: HashMap<String, serde_json::Value>) -> Self {
        self.details = details;
        self
    }

    pub fn with_cause(mut self, cause: impl Into<String>) -> Self {
        self.cause = Some(cause.into());
        self
    }

    pub fn with_trace_id(mut self, trace_id: impl Into<String>) -> Self {
        self.trace_id = Some(trace_id.into());
        self
    }

    pub fn with_ai_guidance(mut self, ai_guidance: impl Into<String>) -> Self {
        self.ai_guidance = Some(ai_guidance.into());
        self
    }

    pub fn with_retryable(mut self, retryable: bool) -> Self {
        self.retryable = Some(retryable);
        self
    }

    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestion = Some(suggestion.into());
        self
    }

    /// Convert to a sparse JSON dictionary (omitting None fields).
    pub fn to_dict(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_else(|_| serde_json::json!({}))
    }

    pub fn config_namespace_duplicate(name: &str) -> Self {
        let mut details = HashMap::new();
        details.insert("name".to_string(), serde_json::json!(name));
        Self::new(
            ErrorCode::ConfigNamespaceDuplicate,
            format!("Namespace '{}' is already registered", name),
        )
        .with_details(details)
    }

    pub fn config_namespace_reserved(name: &str) -> Self {
        let mut details = HashMap::new();
        details.insert("name".to_string(), serde_json::json!(name));
        Self::new(
            ErrorCode::ConfigNamespaceReserved,
            format!("Namespace '{}' is reserved", name),
        )
        .with_details(details)
    }

    pub fn config_env_prefix_conflict(prefix: &str) -> Self {
        let mut details = HashMap::new();
        details.insert("env_prefix".to_string(), serde_json::json!(prefix));
        Self::new(
            ErrorCode::ConfigEnvPrefixConflict,
            format!("env_prefix '{}' conflicts with reserved pattern", prefix),
        )
        .with_details(details)
    }

    pub fn config_env_map_conflict(env_var: &str, owner: &str) -> Self {
        let mut details = HashMap::new();
        details.insert("env_var".to_string(), serde_json::json!(env_var));
        details.insert("owner".to_string(), serde_json::json!(owner));
        Self::new(
            ErrorCode::ConfigEnvMapConflict,
            format!(
                "Environment variable '{}' is already mapped by '{}'",
                env_var, owner
            ),
        )
        .with_details(details)
    }

    pub fn config_mount_error(namespace: &str, reason: &str) -> Self {
        let mut details = HashMap::new();
        details.insert("namespace".to_string(), serde_json::json!(namespace));
        Self::new(
            ErrorCode::ConfigMountError,
            format!("Mount failed for '{}': {}", namespace, reason),
        )
        .with_details(details)
    }

    pub fn config_bind_error(namespace: &str, reason: &str) -> Self {
        let mut details = HashMap::new();
        details.insert("namespace".to_string(), serde_json::json!(namespace));
        Self::new(
            ErrorCode::ConfigBindError,
            format!("Bind failed for '{}': {}", namespace, reason),
        )
        .with_details(details)
    }

    pub fn error_formatter_duplicate(adapter_name: &str) -> Self {
        let mut details = HashMap::new();
        details.insert("adapter_name".to_string(), serde_json::json!(adapter_name));
        Self::new(
            ErrorCode::ErrorFormatterDuplicate,
            format!(
                "ErrorFormatter for adapter '{}' is already registered",
                adapter_name
            ),
        )
        .with_details(details)
    }
}

impl std::fmt::Display for ModuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{:?}] {}", self.code, self.message)
    }
}

impl std::error::Error for ModuleError {}

impl From<serde_json::Error> for ModuleError {
    fn from(err: serde_json::Error) -> Self {
        ModuleError::new(ErrorCode::GeneralInvalidInput, err.to_string())
    }
}

// ---------------------------------------------------------------------------
// Named error types using thiserror
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
#[error("Config not found: {message}")]
pub struct ConfigNotFoundError {
    pub message: String,
    pub config_path: String,
}

impl ConfigNotFoundError {
    pub fn new(message: impl Into<String>, config_path: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            config_path: config_path.into(),
        }
    }

    pub fn to_module_error(&self) -> ModuleError {
        let mut details = HashMap::new();
        details.insert(
            "config_path".to_string(),
            serde_json::Value::String(self.config_path.clone()),
        );
        ModuleError::new(ErrorCode::ConfigNotFound, &self.message).with_details(details)
    }
}

#[derive(Debug, Error)]
#[error("Config invalid: {message}")]
pub struct ConfigInvalidError {
    pub message: String,
}

impl ConfigInvalidError {
    pub fn to_module_error(&self) -> ModuleError {
        ModuleError::new(ErrorCode::ConfigInvalid, &self.message)
    }
}

#[derive(Debug, Error)]
#[error("ACL rule error: {message}")]
pub struct AclRuleError {
    pub message: String,
}

impl AclRuleError {
    pub fn to_module_error(&self) -> ModuleError {
        ModuleError::new(ErrorCode::AclRuleError, &self.message)
    }
}

#[derive(Debug, Error)]
#[error("ACL denied: {message}")]
pub struct AclDeniedError {
    pub message: String,
    pub caller_id: Option<String>,
    pub target_id: String,
}

impl AclDeniedError {
    pub fn new(
        message: impl Into<String>,
        target_id: impl Into<String>,
        caller_id: Option<String>,
    ) -> Self {
        Self {
            message: message.into(),
            caller_id,
            target_id: target_id.into(),
        }
    }

    pub fn to_module_error(&self) -> ModuleError {
        let mut details = HashMap::new();
        details.insert(
            "target_id".to_string(),
            serde_json::Value::String(self.target_id.clone()),
        );
        if let Some(ref caller_id) = self.caller_id {
            details.insert(
                "caller_id".to_string(),
                serde_json::Value::String(caller_id.clone()),
            );
        }
        ModuleError::new(ErrorCode::AclDenied, &self.message)
            .with_details(details)
            .with_ai_guidance(
                "The caller does not have permission to access the target module. \
                 Check ACL rules and ensure the caller has the required permissions.",
            )
    }
}

#[derive(Debug, Error)]
#[error("Module not found: {message}")]
pub struct ModuleNotFoundError {
    pub message: String,
    pub module_id: String,
}

impl ModuleNotFoundError {
    pub fn new(message: impl Into<String>, module_id: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            module_id: module_id.into(),
        }
    }

    pub fn to_module_error(&self) -> ModuleError {
        let mut details = HashMap::new();
        details.insert(
            "module_id".to_string(),
            serde_json::Value::String(self.module_id.clone()),
        );
        ModuleError::new(ErrorCode::ModuleNotFound, &self.message)
            .with_details(details)
            .with_ai_guidance(
                "The requested module is not registered. \
                 Verify the module_id and ensure the module has been registered with the framework.",
            )
    }
}

#[derive(Debug, Error)]
#[error("Module disabled: {message}")]
pub struct ModuleDisabledError {
    pub message: String,
    pub module_id: String,
}

impl ModuleDisabledError {
    pub fn new(message: impl Into<String>, module_id: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            module_id: module_id.into(),
        }
    }

    pub fn to_module_error(&self) -> ModuleError {
        let mut details = HashMap::new();
        details.insert(
            "module_id".to_string(),
            serde_json::Value::String(self.module_id.clone()),
        );
        ModuleError::new(ErrorCode::ModuleDisabled, &self.message)
            .with_details(details)
            .with_ai_guidance(
                "The module is currently disabled. \
                 Enable it in the configuration or check why it was disabled.",
            )
    }
}

#[derive(Debug, Error)]
#[error("Module timeout: {message}")]
pub struct ModuleTimeoutError {
    pub message: String,
    pub module_id: String,
    pub timeout_ms: u64,
}

impl ModuleTimeoutError {
    pub fn new(message: impl Into<String>, module_id: impl Into<String>, timeout_ms: u64) -> Self {
        Self {
            message: message.into(),
            module_id: module_id.into(),
            timeout_ms,
        }
    }

    pub fn to_module_error(&self) -> ModuleError {
        let mut details = HashMap::new();
        details.insert(
            "module_id".to_string(),
            serde_json::Value::String(self.module_id.clone()),
        );
        details.insert(
            "timeout_ms".to_string(),
            serde_json::Value::Number(serde_json::Number::from(self.timeout_ms)),
        );
        ModuleError::new(ErrorCode::ModuleTimeout, &self.message)
            .with_details(details)
            .with_retryable(true)
            .with_ai_guidance(
                "The module execution exceeded the timeout. \
                 Consider increasing the timeout or optimizing the module's execution.",
            )
    }
}

#[derive(Debug, Error)]
#[error("Module load error: {message}")]
pub struct ModuleLoadError {
    pub message: String,
    pub module_id: String,
    pub reason: String,
}

impl ModuleLoadError {
    pub fn new(
        message: impl Into<String>,
        module_id: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            message: message.into(),
            module_id: module_id.into(),
            reason: reason.into(),
        }
    }

    pub fn to_module_error(&self) -> ModuleError {
        let mut details = HashMap::new();
        details.insert(
            "module_id".to_string(),
            serde_json::Value::String(self.module_id.clone()),
        );
        details.insert(
            "reason".to_string(),
            serde_json::Value::String(self.reason.clone()),
        );
        ModuleError::new(ErrorCode::ModuleLoadError, &self.message).with_details(details)
    }
}

#[derive(Debug, Error)]
#[error("Module execute error: {message}")]
pub struct ModuleExecuteError {
    pub message: String,
    pub module_id: String,
}

impl ModuleExecuteError {
    pub fn new(message: impl Into<String>, module_id: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            module_id: module_id.into(),
        }
    }

    pub fn to_module_error(&self) -> ModuleError {
        let mut details = HashMap::new();
        details.insert(
            "module_id".to_string(),
            serde_json::Value::String(self.module_id.clone()),
        );
        ModuleError::new(ErrorCode::ModuleExecuteError, &self.message).with_details(details)
    }
}

#[derive(Debug, Error)]
#[error("Reload failed: {message}")]
pub struct ReloadFailedError {
    pub message: String,
    pub module_id: String,
    pub reason: String,
}

impl ReloadFailedError {
    pub fn new(
        message: impl Into<String>,
        module_id: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            message: message.into(),
            module_id: module_id.into(),
            reason: reason.into(),
        }
    }

    pub fn to_module_error(&self) -> ModuleError {
        let mut details = HashMap::new();
        details.insert(
            "module_id".to_string(),
            serde_json::Value::String(self.module_id.clone()),
        );
        details.insert(
            "reason".to_string(),
            serde_json::Value::String(self.reason.clone()),
        );
        ModuleError::new(ErrorCode::ReloadFailed, &self.message)
            .with_details(details)
            .with_retryable(true)
    }
}

#[derive(Debug, Error)]
#[error("Execution cancelled: {message}")]
pub struct ExecutionCancelledError {
    pub message: String,
}

impl ExecutionCancelledError {
    pub fn to_module_error(&self) -> ModuleError {
        ModuleError::new(ErrorCode::ExecutionCancelled, &self.message)
    }
}

#[derive(Debug, Error)]
#[error("Schema validation error: {message}")]
pub struct SchemaValidationError {
    pub message: String,
    pub errors: Vec<HashMap<String, String>>,
}

impl SchemaValidationError {
    pub fn new(message: impl Into<String>, errors: Vec<HashMap<String, String>>) -> Self {
        Self {
            message: message.into(),
            errors,
        }
    }

    pub fn to_module_error(&self) -> ModuleError {
        let mut details = HashMap::new();
        let errors_json: Vec<serde_json::Value> = self
            .errors
            .iter()
            .map(|e| serde_json::to_value(e).unwrap_or_default())
            .collect();
        details.insert("errors".to_string(), serde_json::Value::Array(errors_json));
        ModuleError::new(ErrorCode::SchemaValidationError, &self.message)
            .with_details(details)
            .with_ai_guidance(
                "Input failed schema validation. \
                 Check the 'errors' field in details for specific validation failures.",
            )
    }
}

#[derive(Debug, Error)]
#[error("Schema not found: {message}")]
pub struct SchemaNotFoundError {
    pub message: String,
    pub schema_id: String,
}

impl SchemaNotFoundError {
    pub fn new(message: impl Into<String>, schema_id: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            schema_id: schema_id.into(),
        }
    }

    pub fn to_module_error(&self) -> ModuleError {
        let mut details = HashMap::new();
        details.insert(
            "schema_id".to_string(),
            serde_json::Value::String(self.schema_id.clone()),
        );
        ModuleError::new(ErrorCode::SchemaNotFound, &self.message).with_details(details)
    }
}

#[derive(Debug, Error)]
#[error("Schema parse error: {message}")]
pub struct SchemaParseError {
    pub message: String,
}

impl SchemaParseError {
    pub fn to_module_error(&self) -> ModuleError {
        ModuleError::new(ErrorCode::SchemaParseError, &self.message)
    }
}

#[derive(Debug, Error)]
#[error("Schema circular ref: {message}")]
pub struct SchemaCircularRefError {
    pub message: String,
    pub ref_path: String,
}

impl SchemaCircularRefError {
    pub fn new(message: impl Into<String>, ref_path: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            ref_path: ref_path.into(),
        }
    }

    pub fn to_module_error(&self) -> ModuleError {
        let mut details = HashMap::new();
        details.insert(
            "ref_path".to_string(),
            serde_json::Value::String(self.ref_path.clone()),
        );
        ModuleError::new(ErrorCode::SchemaCircularRef, &self.message).with_details(details)
    }
}

#[derive(Debug, Error)]
#[error("Call depth exceeded: {message}")]
pub struct CallDepthExceededError {
    pub message: String,
    pub depth: u32,
    pub max_depth: u32,
    pub call_chain: Vec<String>,
}

impl CallDepthExceededError {
    pub fn new(
        message: impl Into<String>,
        depth: u32,
        max_depth: u32,
        call_chain: Vec<String>,
    ) -> Self {
        Self {
            message: message.into(),
            depth,
            max_depth,
            call_chain,
        }
    }

    pub fn to_module_error(&self) -> ModuleError {
        let mut details = HashMap::new();
        details.insert(
            "depth".to_string(),
            serde_json::Value::Number(serde_json::Number::from(self.depth)),
        );
        details.insert(
            "max_depth".to_string(),
            serde_json::Value::Number(serde_json::Number::from(self.max_depth)),
        );
        let chain: Vec<serde_json::Value> = self
            .call_chain
            .iter()
            .map(|s| serde_json::Value::String(s.clone()))
            .collect();
        details.insert("call_chain".to_string(), serde_json::Value::Array(chain));
        ModuleError::new(ErrorCode::CallDepthExceeded, &self.message)
            .with_details(details)
            .with_ai_guidance(
                "The module call depth has exceeded the maximum allowed. \
                 This usually indicates deep or unbounded recursion in module calls.",
            )
    }
}

#[derive(Debug, Error)]
#[error("Circular call detected: {message}")]
pub struct CircularCallError {
    pub message: String,
    pub module_id: String,
    pub call_chain: Vec<String>,
}

impl CircularCallError {
    pub fn new(
        message: impl Into<String>,
        module_id: impl Into<String>,
        call_chain: Vec<String>,
    ) -> Self {
        Self {
            message: message.into(),
            module_id: module_id.into(),
            call_chain,
        }
    }

    pub fn to_module_error(&self) -> ModuleError {
        let mut details = HashMap::new();
        details.insert(
            "module_id".to_string(),
            serde_json::Value::String(self.module_id.clone()),
        );
        let chain: Vec<serde_json::Value> = self
            .call_chain
            .iter()
            .map(|s| serde_json::Value::String(s.clone()))
            .collect();
        details.insert("call_chain".to_string(), serde_json::Value::Array(chain));
        ModuleError::new(ErrorCode::CircularCall, &self.message)
            .with_details(details)
            .with_ai_guidance(
                "A circular call was detected in the module call chain. \
                 Review the call_chain to identify and break the cycle.",
            )
    }
}

#[derive(Debug, Error)]
#[error("Call frequency exceeded: {message}")]
pub struct CallFrequencyExceededError {
    pub message: String,
    pub module_id: String,
    pub count: u32,
    pub max_repeat: u32,
}

impl CallFrequencyExceededError {
    pub fn new(
        message: impl Into<String>,
        module_id: impl Into<String>,
        count: u32,
        max_repeat: u32,
    ) -> Self {
        Self {
            message: message.into(),
            module_id: module_id.into(),
            count,
            max_repeat,
        }
    }

    pub fn to_module_error(&self) -> ModuleError {
        let mut details = HashMap::new();
        details.insert(
            "module_id".to_string(),
            serde_json::Value::String(self.module_id.clone()),
        );
        details.insert(
            "count".to_string(),
            serde_json::Value::Number(serde_json::Number::from(self.count)),
        );
        details.insert(
            "max_repeat".to_string(),
            serde_json::Value::Number(serde_json::Number::from(self.max_repeat)),
        );
        ModuleError::new(ErrorCode::CallFrequencyExceeded, &self.message).with_details(details)
    }
}

#[derive(Debug, Error)]
#[error("Invalid input: {message}")]
pub struct InvalidInputError {
    pub message: String,
}

impl InvalidInputError {
    pub fn to_module_error(&self) -> ModuleError {
        ModuleError::new(ErrorCode::GeneralInvalidInput, &self.message)
    }
}

#[derive(Debug, Error)]
#[error("Internal error: {message}")]
pub struct InternalError {
    pub message: String,
}

impl InternalError {
    pub fn to_module_error(&self) -> ModuleError {
        ModuleError::new(ErrorCode::GeneralInternalError, &self.message).with_retryable(true)
    }
}

#[derive(Debug, Error)]
#[error("Not implemented: {message}")]
pub struct FeatureNotImplementedError {
    pub message: String,
}

impl FeatureNotImplementedError {
    pub fn to_module_error(&self) -> ModuleError {
        ModuleError::new(ErrorCode::GeneralNotImplemented, &self.message)
    }
}

#[derive(Debug, Error)]
#[error("Missing type hint: {message}")]
pub struct FuncMissingTypeHintError {
    pub message: String,
    pub function_name: String,
    pub parameter_name: String,
}

impl FuncMissingTypeHintError {
    pub fn new(
        message: impl Into<String>,
        function_name: impl Into<String>,
        parameter_name: impl Into<String>,
    ) -> Self {
        Self {
            message: message.into(),
            function_name: function_name.into(),
            parameter_name: parameter_name.into(),
        }
    }

    pub fn to_module_error(&self) -> ModuleError {
        let mut details = HashMap::new();
        details.insert(
            "function_name".to_string(),
            serde_json::Value::String(self.function_name.clone()),
        );
        details.insert(
            "parameter_name".to_string(),
            serde_json::Value::String(self.parameter_name.clone()),
        );
        ModuleError::new(ErrorCode::FuncMissingTypeHint, &self.message).with_details(details)
    }
}

#[derive(Debug, Error)]
#[error("Missing return type: {message}")]
pub struct FuncMissingReturnTypeError {
    pub message: String,
    pub function_name: String,
}

impl FuncMissingReturnTypeError {
    pub fn new(message: impl Into<String>, function_name: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            function_name: function_name.into(),
        }
    }

    pub fn to_module_error(&self) -> ModuleError {
        let mut details = HashMap::new();
        details.insert(
            "function_name".to_string(),
            serde_json::Value::String(self.function_name.clone()),
        );
        ModuleError::new(ErrorCode::FuncMissingReturnType, &self.message).with_details(details)
    }
}

#[derive(Debug, Error)]
#[error("Binding invalid target: {message}")]
pub struct BindingInvalidTargetError {
    pub message: String,
    pub target: String,
}

impl BindingInvalidTargetError {
    pub fn new(message: impl Into<String>, target: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            target: target.into(),
        }
    }

    pub fn to_module_error(&self) -> ModuleError {
        let mut details = HashMap::new();
        details.insert(
            "target".to_string(),
            serde_json::Value::String(self.target.clone()),
        );
        ModuleError::new(ErrorCode::BindingInvalidTarget, &self.message).with_details(details)
    }
}

#[derive(Debug, Error)]
#[error("Binding module not found: {message}")]
pub struct BindingModuleNotFoundError {
    pub message: String,
    pub module_path: String,
}

impl BindingModuleNotFoundError {
    pub fn new(message: impl Into<String>, module_path: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            module_path: module_path.into(),
        }
    }

    pub fn to_module_error(&self) -> ModuleError {
        let mut details = HashMap::new();
        details.insert(
            "module_path".to_string(),
            serde_json::Value::String(self.module_path.clone()),
        );
        ModuleError::new(ErrorCode::BindingModuleNotFound, &self.message).with_details(details)
    }
}

#[derive(Debug, Error)]
#[error("Binding callable not found: {message}")]
pub struct BindingCallableNotFoundError {
    pub message: String,
    pub callable_name: String,
    pub module_path: String,
}

impl BindingCallableNotFoundError {
    pub fn new(
        message: impl Into<String>,
        callable_name: impl Into<String>,
        module_path: impl Into<String>,
    ) -> Self {
        Self {
            message: message.into(),
            callable_name: callable_name.into(),
            module_path: module_path.into(),
        }
    }

    pub fn to_module_error(&self) -> ModuleError {
        let mut details = HashMap::new();
        details.insert(
            "callable_name".to_string(),
            serde_json::Value::String(self.callable_name.clone()),
        );
        details.insert(
            "module_path".to_string(),
            serde_json::Value::String(self.module_path.clone()),
        );
        ModuleError::new(ErrorCode::BindingCallableNotFound, &self.message).with_details(details)
    }
}

#[derive(Debug, Error)]
#[error("Binding not callable: {message}")]
pub struct BindingNotCallableError {
    pub message: String,
    pub target: String,
}

impl BindingNotCallableError {
    pub fn new(message: impl Into<String>, target: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            target: target.into(),
        }
    }

    pub fn to_module_error(&self) -> ModuleError {
        let mut details = HashMap::new();
        details.insert(
            "target".to_string(),
            serde_json::Value::String(self.target.clone()),
        );
        ModuleError::new(ErrorCode::BindingNotCallable, &self.message).with_details(details)
    }
}

#[derive(Debug, Error)]
#[error("Binding schema missing: {message}")]
pub struct BindingSchemaMissingError {
    pub message: String,
    pub target: String,
}

impl BindingSchemaMissingError {
    pub fn new(message: impl Into<String>, target: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            target: target.into(),
        }
    }

    pub fn to_module_error(&self) -> ModuleError {
        let mut details = HashMap::new();
        details.insert(
            "target".to_string(),
            serde_json::Value::String(self.target.clone()),
        );
        ModuleError::new(ErrorCode::BindingSchemaMissing, &self.message).with_details(details)
    }
}

#[derive(Debug, Error)]
#[error("Binding file invalid: {message}")]
pub struct BindingFileInvalidError {
    pub message: String,
    pub file_path: String,
    pub reason: String,
}

impl BindingFileInvalidError {
    pub fn new(
        message: impl Into<String>,
        file_path: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            message: message.into(),
            file_path: file_path.into(),
            reason: reason.into(),
        }
    }

    pub fn to_module_error(&self) -> ModuleError {
        let mut details = HashMap::new();
        details.insert(
            "file_path".to_string(),
            serde_json::Value::String(self.file_path.clone()),
        );
        details.insert(
            "reason".to_string(),
            serde_json::Value::String(self.reason.clone()),
        );
        ModuleError::new(ErrorCode::BindingFileInvalid, &self.message).with_details(details)
    }
}

#[derive(Debug, Error)]
#[error("Circular dependency: {message}")]
pub struct CircularDependencyError {
    pub message: String,
    pub cycle_path: Vec<String>,
}

impl CircularDependencyError {
    pub fn new(message: impl Into<String>, cycle_path: Vec<String>) -> Self {
        Self {
            message: message.into(),
            cycle_path,
        }
    }

    pub fn to_module_error(&self) -> ModuleError {
        let mut details = HashMap::new();
        let path: Vec<serde_json::Value> = self
            .cycle_path
            .iter()
            .map(|s| serde_json::Value::String(s.clone()))
            .collect();
        details.insert("cycle_path".to_string(), serde_json::Value::Array(path));
        ModuleError::new(ErrorCode::CircularDependency, &self.message).with_details(details)
    }
}

#[derive(Debug, Error)]
#[error("Middleware chain error: {message}")]
pub struct MiddlewareChainError {
    pub message: String,
}

impl MiddlewareChainError {
    pub fn to_module_error(&self) -> ModuleError {
        ModuleError::new(ErrorCode::MiddlewareChainError, &self.message)
    }
}

#[derive(Debug, Error)]
#[error("Approval denied: {message}")]
pub struct ApprovalDeniedError {
    pub message: String,
    pub module_id: String,
    pub reason: Option<String>,
}

impl ApprovalDeniedError {
    pub fn new(
        message: impl Into<String>,
        module_id: impl Into<String>,
        reason: Option<String>,
    ) -> Self {
        Self {
            message: message.into(),
            module_id: module_id.into(),
            reason,
        }
    }

    pub fn to_module_error(&self) -> ModuleError {
        let mut details = HashMap::new();
        details.insert(
            "module_id".to_string(),
            serde_json::Value::String(self.module_id.clone()),
        );
        if let Some(ref reason) = self.reason {
            details.insert(
                "reason".to_string(),
                serde_json::Value::String(reason.clone()),
            );
        }
        ModuleError::new(ErrorCode::ApprovalDenied, &self.message).with_details(details)
    }
}

#[derive(Debug, Error)]
#[error("Approval timeout: {message}")]
pub struct ApprovalTimeoutError {
    pub message: String,
    pub module_id: String,
}

impl ApprovalTimeoutError {
    pub fn new(message: impl Into<String>, module_id: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            module_id: module_id.into(),
        }
    }

    pub fn to_module_error(&self) -> ModuleError {
        let mut details = HashMap::new();
        details.insert(
            "module_id".to_string(),
            serde_json::Value::String(self.module_id.clone()),
        );
        ModuleError::new(ErrorCode::ApprovalTimeout, &self.message)
            .with_details(details)
            .with_retryable(true)
    }
}

#[derive(Debug, Error)]
#[error("Approval pending: {message}")]
pub struct ApprovalPendingError {
    pub message: String,
    pub module_id: String,
    pub approval_id: Option<String>,
}

impl ApprovalPendingError {
    pub fn new(
        message: impl Into<String>,
        module_id: impl Into<String>,
        approval_id: Option<String>,
    ) -> Self {
        Self {
            message: message.into(),
            module_id: module_id.into(),
            approval_id,
        }
    }

    pub fn to_module_error(&self) -> ModuleError {
        let mut details = HashMap::new();
        details.insert(
            "module_id".to_string(),
            serde_json::Value::String(self.module_id.clone()),
        );
        if let Some(ref approval_id) = self.approval_id {
            details.insert(
                "approval_id".to_string(),
                serde_json::Value::String(approval_id.clone()),
            );
        }
        ModuleError::new(ErrorCode::ApprovalPending, &self.message).with_details(details)
    }
}

#[derive(Debug, Error)]
#[error("Version incompatible: {message}")]
pub struct VersionIncompatibleError {
    pub message: String,
}

impl VersionIncompatibleError {
    pub fn to_module_error(&self) -> ModuleError {
        ModuleError::new(ErrorCode::VersionIncompatible, &self.message)
    }
}

#[derive(Debug, Error)]
#[error("Error code collision: {message}")]
pub struct ErrorCodeCollisionError {
    pub message: String,
    pub code: String,
    pub module_id: String,
    pub conflict_source: String,
}

impl ErrorCodeCollisionError {
    pub fn new(
        message: impl Into<String>,
        code: impl Into<String>,
        module_id: impl Into<String>,
        conflict_source: impl Into<String>,
    ) -> Self {
        Self {
            message: message.into(),
            code: code.into(),
            module_id: module_id.into(),
            conflict_source: conflict_source.into(),
        }
    }

    pub fn to_module_error(&self) -> ModuleError {
        let mut details = HashMap::new();
        details.insert(
            "code".to_string(),
            serde_json::Value::String(self.code.clone()),
        );
        details.insert(
            "module_id".to_string(),
            serde_json::Value::String(self.module_id.clone()),
        );
        details.insert(
            "conflict_source".to_string(),
            serde_json::Value::String(self.conflict_source.clone()),
        );
        ModuleError::new(ErrorCode::ErrorCodeCollision, &self.message).with_details(details)
    }
}

#[derive(Debug, Error)]
#[error("Dependency not found: {message}")]
pub struct DependencyNotFoundError {
    pub message: String,
    pub module_id: String,
    pub dependency_id: String,
}

impl DependencyNotFoundError {
    pub fn new(
        message: impl Into<String>,
        module_id: impl Into<String>,
        dependency_id: impl Into<String>,
    ) -> Self {
        Self {
            message: message.into(),
            module_id: module_id.into(),
            dependency_id: dependency_id.into(),
        }
    }

    pub fn to_module_error(&self) -> ModuleError {
        let mut details = HashMap::new();
        details.insert(
            "module_id".to_string(),
            serde_json::Value::String(self.module_id.clone()),
        );
        details.insert(
            "dependency_id".to_string(),
            serde_json::Value::String(self.dependency_id.clone()),
        );
        ModuleError::new(ErrorCode::DependencyNotFound, &self.message).with_details(details)
    }
}

// ---------------------------------------------------------------------------
// ErrorCodeRegistry — tracks module error codes and detects collisions
// ---------------------------------------------------------------------------

/// Registry that tracks which error codes belong to which modules,
/// enforcing framework-prefix reservation and cross-module uniqueness.
///
/// Matches the Python `ErrorCodeRegistry` for conformance testing.
pub struct ErrorCodeRegistry {
    module_codes: HashMap<String, HashSet<String>>,
    all_codes: HashSet<String>,
}

impl ErrorCodeRegistry {
    pub fn new() -> Self {
        Self {
            module_codes: HashMap::new(),
            all_codes: HashSet::new(),
        }
    }

    /// Register a set of error codes for the given module.
    ///
    /// Returns `Err(ModuleError)` with `ErrorCode::ErrorCodeCollision` if any
    /// code uses a reserved framework prefix or is already owned by a different
    /// module. Re-registering the same code for the same module is idempotent.
    pub fn register(
        &mut self,
        module_id: &str,
        codes: &HashSet<String>,
    ) -> Result<(), ModuleError> {
        for code in codes {
            // Check framework prefix collision
            for prefix in FRAMEWORK_ERROR_CODE_PREFIXES {
                if code.starts_with(prefix) {
                    return Err(ErrorCodeCollisionError::new(
                        format!(
                            "Error code '{}' uses reserved framework prefix '{}'",
                            code, prefix
                        ),
                        code,
                        module_id,
                        "framework",
                    )
                    .to_module_error());
                }
            }

            // Check cross-module collision
            if let Some(owner) = self.find_owner(code) {
                if owner != module_id {
                    return Err(ErrorCodeCollisionError::new(
                        format!(
                            "Error code '{}' already registered by module '{}'",
                            code, owner
                        ),
                        code,
                        module_id,
                        &owner,
                    )
                    .to_module_error());
                }
            }
        }

        let existing = self
            .module_codes
            .entry(module_id.to_string())
            .or_default();
        existing.extend(codes.iter().cloned());

        self.rebuild_all_codes();
        Ok(())
    }

    /// Remove all error codes registered for the given module.
    pub fn unregister(&mut self, module_id: &str) {
        self.module_codes.remove(module_id);
        self.rebuild_all_codes();
    }

    /// Find the module that owns the given error code, if any.
    fn find_owner(&self, code: &str) -> Option<String> {
        for (mid, codes) in &self.module_codes {
            if codes.contains(code) {
                return Some(mid.clone());
            }
        }
        None
    }

    /// Rebuild the `all_codes` set from the current `module_codes`.
    fn rebuild_all_codes(&mut self) {
        self.all_codes = self
            .module_codes
            .values()
            .flat_map(|codes| codes.iter().cloned())
            .collect();
    }

    /// Returns a reference to the set of all currently registered codes.
    pub fn all_codes(&self) -> &HashSet<String> {
        &self.all_codes
    }

    /// Returns the codes registered for a specific module, if any.
    pub fn codes_for_module(&self, module_id: &str) -> Option<&HashSet<String>> {
        self.module_codes.get(module_id)
    }
}

impl Default for ErrorCodeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

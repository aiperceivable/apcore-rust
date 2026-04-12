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
    "PIPELINE_",
    "STEP_",
    "STRATEGY_",
];

/// All error codes defined by the APCore protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    ConfigNotFound,
    ConfigInvalid,
    #[serde(rename = "ACL_RULE_ERROR")]
    ACLRuleError,
    #[serde(rename = "ACL_DENIED")]
    ACLDenied,
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
    PipelineAbort,
    StepNotFound,
    StepNotRemovable,
    StepNotReplaceable,
    StepNameDuplicate,
    StrategyNotFound,
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

    #[must_use]
    pub fn with_details(mut self, details: HashMap<String, serde_json::Value>) -> Self {
        self.details = details;
        self
    }

    #[must_use]
    pub fn with_cause(mut self, cause: impl Into<String>) -> Self {
        self.cause = Some(cause.into());
        self
    }

    #[must_use]
    pub fn with_trace_id(mut self, trace_id: impl Into<String>) -> Self {
        self.trace_id = Some(trace_id.into());
        self
    }

    #[must_use]
    pub fn with_ai_guidance(mut self, ai_guidance: impl Into<String>) -> Self {
        self.ai_guidance = Some(ai_guidance.into());
        self
    }

    #[must_use]
    pub fn with_retryable(mut self, retryable: bool) -> Self {
        self.retryable = Some(retryable);
        self
    }

    #[must_use]
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
            format!("Namespace '{name}' is already registered"),
        )
        .with_details(details)
    }

    pub fn config_namespace_reserved(name: &str) -> Self {
        let mut details = HashMap::new();
        details.insert("name".to_string(), serde_json::json!(name));
        Self::new(
            ErrorCode::ConfigNamespaceReserved,
            format!("Namespace '{name}' is reserved"),
        )
        .with_details(details)
    }

    pub fn config_env_prefix_conflict(prefix: &str) -> Self {
        let mut details = HashMap::new();
        details.insert("env_prefix".to_string(), serde_json::json!(prefix));
        Self::new(
            ErrorCode::ConfigEnvPrefixConflict,
            format!("env_prefix '{prefix}' conflicts with reserved pattern"),
        )
        .with_details(details)
    }

    pub fn config_env_map_conflict(env_var: &str, owner: &str) -> Self {
        let mut details = HashMap::new();
        details.insert("env_var".to_string(), serde_json::json!(env_var));
        details.insert("owner".to_string(), serde_json::json!(owner));
        Self::new(
            ErrorCode::ConfigEnvMapConflict,
            format!("Environment variable '{env_var}' is already mapped by '{owner}'"),
        )
        .with_details(details)
    }

    pub fn config_mount_error(namespace: &str, reason: &str) -> Self {
        let mut details = HashMap::new();
        details.insert("namespace".to_string(), serde_json::json!(namespace));
        Self::new(
            ErrorCode::ConfigMountError,
            format!("Mount failed for '{namespace}': {reason}"),
        )
        .with_details(details)
    }

    pub fn config_bind_error(namespace: &str, reason: &str) -> Self {
        let mut details = HashMap::new();
        details.insert("namespace".to_string(), serde_json::json!(namespace));
        Self::new(
            ErrorCode::ConfigBindError,
            format!("Bind failed for '{namespace}': {reason}"),
        )
        .with_details(details)
    }

    pub fn error_formatter_duplicate(adapter_name: &str) -> Self {
        let mut details = HashMap::new();
        details.insert("adapter_name".to_string(), serde_json::json!(adapter_name));
        Self::new(
            ErrorCode::ErrorFormatterDuplicate,
            format!("ErrorFormatter for adapter '{adapter_name}' is already registered"),
        )
        .with_details(details)
    }

    pub fn pipeline_abort(step: &str, explanation: Option<&str>) -> Self {
        let mut details = HashMap::new();
        details.insert("step".to_string(), serde_json::json!(step));
        Self::new(
            ErrorCode::PipelineAbort,
            format!(
                "Pipeline aborted at step '{}': {}",
                step,
                explanation.unwrap_or("no explanation")
            ),
        )
        .with_details(details)
    }

    pub fn step_not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::StepNotFound, message)
    }

    pub fn step_not_removable(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::StepNotRemovable, message)
    }

    pub fn step_not_replaceable(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::StepNotReplaceable, message)
    }

    pub fn step_name_duplicate(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::StepNameDuplicate, message)
    }

    pub fn strategy_not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::StrategyNotFound, message)
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
                        format!("Error code '{code}' uses reserved framework prefix '{prefix}'"),
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
                        format!("Error code '{code}' already registered by module '{owner}'"),
                        code,
                        module_id,
                        &owner,
                    )
                    .to_module_error());
                }
            }
        }

        let existing = self.module_codes.entry(module_id.to_string()).or_default();
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

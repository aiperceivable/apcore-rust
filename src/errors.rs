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
    "CIRCUIT_",
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

/// All error codes defined by the `APCore` protocol.
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
    /// Issue #44 (PROTOCOL_SPEC §4.15): a `oneOf` or `anyOf` schema rejected the
    /// input because no branch matched. Cross-language: Python/TS `SCHEMA_UNION_NO_MATCH`.
    SchemaUnionNoMatch,
    /// Issue #44 (PROTOCOL_SPEC §4.15): a `oneOf` schema rejected the input because
    /// more than one branch matched. Cross-language: Python/TS `SCHEMA_UNION_AMBIGUOUS`.
    SchemaUnionAmbiguous,
    /// Issue #44 (PROTOCOL_SPEC §4.15): recursive `$ref` resolution exceeded `max_depth`.
    /// Cross-language: Python/TS `SCHEMA_MAX_DEPTH_EXCEEDED`.
    SchemaMaxDepthExceeded,
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
    /// Deprecated since 0.19.0; superseded by `BindingSchemaInferenceFailed`.
    /// Kept for backward-compatibility deserialization. See `DECLARATIVE_CONFIG_SPEC.md` §7.1.
    /// The alias below makes the backward-compat contract explicit even if the variant is ever
    /// renamed: old serialized payloads containing `"BINDING_SCHEMA_MISSING"` remain decodable.
    #[serde(alias = "BINDING_SCHEMA_MISSING")]
    BindingSchemaMissing,
    BindingSchemaInferenceFailed,
    BindingSchemaModeConflict,
    BindingStrictSchemaIncompatible,
    BindingFileInvalid,
    CircularDependency,
    MiddlewareChainError,
    ApprovalDenied,
    ApprovalTimeout,
    ApprovalPending,
    VersionIncompatible,
    ErrorCodeCollision,
    DependencyNotFound,
    DependencyVersionMismatch,
    ConfigNamespaceDuplicate,
    ConfigNamespaceReserved,
    ConfigEnvPrefixConflict,
    ConfigMountError,
    ConfigBindError,
    ConfigEnvMapConflict,
    ErrorFormatterDuplicate,
    PipelineAbort,
    PipelineConfigInvalid,
    /// Sync alignment (W-7): pipeline-configuration errors that are NOT
    /// dependency-graph violations (`requires`/`provides`) — for example
    /// removing a non-existent step, configuring a non-existent step, or
    /// declaring a custom step without a valid `after`/`before` anchor.
    /// Distinct from [`Self::PipelineDependencyError`] so callers can match
    /// the structural-config case independently of dependency-graph failures.
    /// Cross-language: Python/TS `CONFIGURATION_ERROR`.
    ConfigurationError,
    PipelineHandlerNotSupported,
    PipelineStepInsertionAmbiguous,
    /// Issue #33 (core-executor.md §Pipeline Hardening §1.1): a pipeline step's
    /// handler raised an error and `ignore_errors` is `false`. The step name and
    /// the original error are stored in `details["step_name"]` / `details["cause"]`.
    /// Cross-language: Python/TS `PIPELINE_STEP_ERROR`.
    PipelineStepError,
    /// Issue #33 (core-executor.md §Pipeline Hardening §1.2): `configure_step`
    /// targeted a step name that does not exist in the strategy.
    /// Cross-language: Python/TS `PIPELINE_STEP_NOT_FOUND`.
    PipelineStepNotFound,
    /// Issue #33 §2.1: a strategy was constructed in which a step's
    /// `requires` list referenced a field not produced by any preceding
    /// step's `provides`. Strategies MUST fail fast at construction rather
    /// than warn at runtime. Details carry `step_name` and `requires`.
    /// Cross-language: Python/TS `PIPELINE_DEPENDENCY_ERROR`.
    PipelineDependencyError,
    StepNotFound,
    StepNotRemovable,
    StepNotReplaceable,
    StepNameDuplicate,
    StrategyNotFound,
    EntryPointNotFound,
    EntryPointAmbiguous,
    /// Reserved for future opt-in runtime entry-point loading APIs (e.g.,
    /// `libloading`-based plugin discovery). No current API path raises this
    /// error. See `DECLARATIVE_CONFIG_SPEC.md` §5.2.
    EntryPointRuntimeUnsupported,
    /// `Registry::discover_internal()` was called but no custom discoverer
    /// has been configured via `Registry::set_discoverer()`. Rust-specific:
    /// `apcore-python` has a default filesystem discoverer so this state is
    /// unreachable there.
    NoDiscovererConfigured,
    /// Raised when `AsyncTaskManager::submit` is called at the task-slot limit.
    /// Cross-language: Python `TASK_LIMIT_EXCEEDED`, TypeScript `TASK_LIMIT_EXCEEDED`.
    TaskLimitExceeded,
    /// Raised when a version constraint string is malformed (e.g., `">="`
    /// without a digit operand, `"v1.0"` prefix, or a non-semver operand).
    /// Cross-language: Python `VERSION_CONSTRAINT_INVALID`, TypeScript `VERSION_CONSTRAINT_INVALID`.
    VersionConstraintInvalid,
    /// Issue #32 (PROTOCOL_SPEC §2.1.1, multi-module-discovery.md): two or more
    /// classes in the same file produce the same `class_segment` after
    /// `snake_case` conversion. The registry rejects the entire file — no
    /// partial registration is permitted. Details carry `file_path`,
    /// `class_names`, and `conflicting_segment`. Cross-language:
    /// Python/TS `MODULE_ID_CONFLICT`.
    ModuleIdConflict,
    /// Issue #32 (PROTOCOL_SPEC §2.1.1): a derived `class_segment` does not
    /// conform to the canonical ID grammar (e.g., starts with a digit after
    /// snake_case conversion). Details carry `file_path`, `class_name`, and
    /// `segment`. Cross-language: Python/TS `INVALID_SEGMENT`.
    InvalidSegment,
    /// Issue #32 (PROTOCOL_SPEC §2.1.1, §2.7): the full derived `module_id`
    /// exceeds `MAX_MODULE_ID_LENGTH` (192 characters). Details carry
    /// `file_path` and `module_id`. Cross-language: Python/TS `ID_TOO_LONG`.
    IdTooLong,
    /// Issue #42 (middleware-system.md §1.2): the `CircuitBreakerMiddleware`
    /// short-circuited a call because the circuit for the (`module_id`,
    /// `caller_id`) pair is `OPEN`. Details carry `module_id` and `caller_id`.
    /// Cross-language: Python/TS `CIRCUIT_BREAKER_OPEN`.
    #[serde(rename = "CIRCUIT_BREAKER_OPEN")]
    CircuitBreakerOpen,
    /// Issue #45 (system-modules.md §1.4): `system.control.reload_module` was
    /// called with both `module_id` and `path_filter` set. Cross-language:
    /// Python/TS `MODULE_RELOAD_CONFLICT`.
    ModuleReloadConflict,
    /// Issue #45 (system-modules.md §1.5): a system module failed to register
    /// during `register_sys_modules` and the caller requested strict failure.
    /// Cross-language: Python/TS `SYS_MODULE_REGISTRATION_FAILED`.
    SysModuleRegistrationFailed,
    /// `APCore::disable()` / `APCore::enable()` was called but `sys_modules` is
    /// not enabled in the current config. Cross-language: Python `RuntimeError`,
    /// TypeScript `Error` (sync finding A-007). The Rust SDK uses a typed
    /// `ModuleError` with this code rather than panicking, matching idiomatic
    /// Rust error handling.
    SysModulesDisabled,
    /// Sync finding D-25: `system.control.update_config` was called with a key
    /// listed in `RESTRICTED_KEYS`. Cross-language: Python/TS
    /// `CONFIG_KEY_RESTRICTED`. Distinct from `ConfigInvalid` so consumers
    /// can match the policy-deny case separately from value-shape errors.
    ConfigKeyRestricted,
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
    #[must_use]
    pub fn to_dict(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_else(|_| serde_json::json!({}))
    }

    #[must_use]
    pub fn config_namespace_duplicate(name: &str) -> Self {
        let mut details = HashMap::new();
        details.insert("name".to_string(), serde_json::json!(name));
        Self::new(
            ErrorCode::ConfigNamespaceDuplicate,
            format!("Namespace '{name}' is already registered"),
        )
        .with_details(details)
    }

    #[must_use]
    pub fn config_namespace_reserved(name: &str) -> Self {
        let mut details = HashMap::new();
        details.insert("name".to_string(), serde_json::json!(name));
        Self::new(
            ErrorCode::ConfigNamespaceReserved,
            format!("Namespace '{name}' is reserved"),
        )
        .with_details(details)
    }

    #[must_use]
    pub fn config_env_prefix_conflict(prefix: &str) -> Self {
        let mut details = HashMap::new();
        details.insert("env_prefix".to_string(), serde_json::json!(prefix));
        Self::new(
            ErrorCode::ConfigEnvPrefixConflict,
            format!("env_prefix '{prefix}' conflicts with reserved pattern"),
        )
        .with_details(details)
    }

    #[must_use]
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

    #[must_use]
    pub fn config_mount_error(namespace: &str, reason: &str) -> Self {
        let mut details = HashMap::new();
        details.insert("namespace".to_string(), serde_json::json!(namespace));
        Self::new(
            ErrorCode::ConfigMountError,
            format!("Mount failed for '{namespace}': {reason}"),
        )
        .with_details(details)
    }

    #[must_use]
    pub fn config_bind_error(namespace: &str, reason: &str) -> Self {
        let mut details = HashMap::new();
        details.insert("namespace".to_string(), serde_json::json!(namespace));
        Self::new(
            ErrorCode::ConfigBindError,
            format!("Bind failed for '{namespace}': {reason}"),
        )
        .with_details(details)
    }

    #[must_use]
    pub fn error_formatter_duplicate(adapter_name: &str) -> Self {
        let mut details = HashMap::new();
        details.insert("adapter_name".to_string(), serde_json::json!(adapter_name));
        Self::new(
            ErrorCode::ErrorFormatterDuplicate,
            format!("ErrorFormatter for adapter '{adapter_name}' is already registered"),
        )
        .with_details(details)
    }

    #[must_use]
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

    /// Wrap a step's underlying error in a `PipelineStepError` per §1.1.
    ///
    /// The original error is preserved in `details["cause"]` (full JSON form) and
    /// the step name in `details["step_name"]`. Use [`Self::is_pipeline_step_error`]
    /// + [`Self::unwrap_pipeline_step_error`] to recover the original error.
    #[must_use]
    pub fn pipeline_step_error(step_name: &str, cause: &ModuleError) -> Self {
        let mut details = HashMap::new();
        details.insert("step_name".to_string(), serde_json::json!(step_name));
        if let Ok(cause_json) = serde_json::to_value(cause) {
            details.insert("cause".to_string(), cause_json);
        }
        let cause_message = cause.message.clone();
        Self::new(
            ErrorCode::PipelineStepError,
            format!("Pipeline step '{step_name}' failed: {cause_message}"),
        )
        .with_details(details)
        .with_cause(cause_message)
    }

    /// Whether this error is a `PipelineStepError` wrapper.
    #[must_use]
    pub fn is_pipeline_step_error(&self) -> bool {
        self.code == ErrorCode::PipelineStepError
    }

    /// The step name carried by a `PipelineStepError`, if any.
    #[must_use]
    pub fn step_name(&self) -> Option<&str> {
        self.details.get("step_name").and_then(|v| v.as_str())
    }

    /// Recover the original error wrapped by `pipeline_step_error()`. Returns
    /// `None` if this error is not a `PipelineStepError` or the cause was not
    /// preserved in a structured form.
    #[must_use]
    pub fn unwrap_pipeline_step_error(&self) -> Option<ModuleError> {
        if !self.is_pipeline_step_error() {
            return None;
        }
        self.details
            .get("cause")
            .and_then(|v| serde_json::from_value::<ModuleError>(v.clone()).ok())
    }

    /// Returns true if this error is a `MiddlewareChainError`.
    #[must_use]
    pub fn is_middleware_chain_error(&self) -> bool {
        matches!(self.code, ErrorCode::MiddlewareChainError)
    }

    /// Recover the original error wrapped by `MiddlewareManager.execute_before()`.
    ///
    /// Returns `None` if this error is not a `MiddlewareChainError` or the
    /// inner error was not preserved in a structured form. Cross-language
    /// parity with apcore-python `MiddlewareChainError.original` and
    /// apcore-typescript `MiddlewareChainError.original` (sync finding A-D-015).
    #[must_use]
    pub fn unwrap_middleware_chain_error(&self) -> Option<ModuleError> {
        if !self.is_middleware_chain_error() {
            return None;
        }
        self.details
            .get("inner_error")
            .and_then(|v| serde_json::from_value::<ModuleError>(v.clone()).ok())
    }

    /// Builder for `MODULE_ID_CONFLICT` (Issue #32, PROTOCOL_SPEC §2.1.1).
    ///
    /// Two or more classes in the same file produced the same `class_segment`
    /// after snake_case conversion. The whole file is rejected.
    #[must_use]
    pub fn module_id_conflict(
        file_path: &str,
        class_names: &[String],
        conflicting_segment: &str,
    ) -> Self {
        let mut details = HashMap::new();
        details.insert("file_path".to_string(), serde_json::json!(file_path));
        details.insert("class_names".to_string(), serde_json::json!(class_names));
        details.insert(
            "conflicting_segment".to_string(),
            serde_json::json!(conflicting_segment),
        );
        Self::new(
            ErrorCode::ModuleIdConflict,
            format!(
                "Module ID conflict in '{file_path}': classes {class_names:?} both produce segment '{conflicting_segment}'"
            ),
        )
        .with_details(details)
    }

    /// Builder for `INVALID_SEGMENT` (Issue #32, PROTOCOL_SPEC §2.1.1).
    #[must_use]
    pub fn invalid_segment(file_path: &str, class_name: &str, segment: &str) -> Self {
        let mut details = HashMap::new();
        details.insert("file_path".to_string(), serde_json::json!(file_path));
        details.insert("class_name".to_string(), serde_json::json!(class_name));
        details.insert("segment".to_string(), serde_json::json!(segment));
        Self::new(
            ErrorCode::InvalidSegment,
            format!(
                "Invalid class segment '{segment}' derived from '{class_name}' in '{file_path}': must match \
                 ^[a-z][a-z0-9_]*$"
            ),
        )
        .with_details(details)
    }

    /// Builder for `CIRCUIT_OPEN` (Issue #42, middleware-system.md §1.2).
    ///
    /// Returned by `CircuitBreakerMiddleware::before` when the circuit for the
    /// given `(module_id, caller_id)` pair is `OPEN`. The error carries
    /// `details["module_id"]` and `details["caller_id"]`.
    #[must_use]
    pub fn circuit_breaker_open(module_id: &str, caller_id: &str) -> Self {
        let mut details = HashMap::new();
        details.insert("module_id".to_string(), serde_json::json!(module_id));
        details.insert("caller_id".to_string(), serde_json::json!(caller_id));
        Self::new(
            ErrorCode::CircuitBreakerOpen,
            format!("Circuit open for module '{module_id}' (caller '{caller_id}') — call rejected"),
        )
        .with_details(details)
        .with_retryable(true)
        .with_ai_guidance(
            "The downstream module is temporarily unavailable. The circuit will probe \
             after the recovery window elapses; retry the request after a short delay.",
        )
    }

    /// Builder for `ID_TOO_LONG` (Issue #32, PROTOCOL_SPEC §2.1.1, §2.7).
    #[must_use]
    pub fn id_too_long(file_path: &str, module_id: &str) -> Self {
        let mut details = HashMap::new();
        details.insert("file_path".to_string(), serde_json::json!(file_path));
        details.insert("module_id".to_string(), serde_json::json!(module_id));
        Self::new(
            ErrorCode::IdTooLong,
            format!(
                "Derived module_id '{module_id}' exceeds maximum length of {} characters",
                crate::registry::multi_class::MAX_MODULE_ID_LEN
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

    #[must_use]
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

    #[must_use]
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
    #[must_use]
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

    #[must_use]
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
    #[must_use]
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
    #[must_use]
    pub fn all_codes(&self) -> &HashSet<String> {
        &self.all_codes
    }

    /// Returns the codes registered for a specific module, if any.
    #[must_use]
    pub fn codes_for_module(&self, module_id: &str) -> Option<&HashSet<String>> {
        self.module_codes.get(module_id)
    }
}

impl Default for ErrorCodeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

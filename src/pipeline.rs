// APCore Protocol — Execution pipeline types
// Spec reference: design-execution-pipeline.md (Sections 2, 3.3, 8.2)

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::acl::ACL;
use crate::approval::ApprovalHandler;
use crate::config::Config;
use crate::context::Context;
use crate::errors::{ErrorCode, ModuleError};
use crate::middleware::manager::MiddlewareManager;
use crate::module::Module;
use crate::registry::registry::Registry;
use crate::utils::helpers::match_pattern;

/// Predicate evaluated by [`PipelineEngine`] after each step completes.
/// Returning `true` halts the pipeline; subsequent steps are not executed.
/// See `core-executor.md` §Pipeline Hardening §1.4.
pub type RunUntilPredicate = Box<dyn Fn(&PipelineState) -> bool + Send + Sync>;

// ---------------------------------------------------------------------------
// StepMiddleware
// ---------------------------------------------------------------------------

/// Step-scoped interceptor invoked around every step in an [`ExecutionStrategy`].
///
/// `StepMiddleware` complements the global [`crate::middleware::Middleware`] trait,
/// which wraps the entire module call. `StepMiddleware` instead wraps each
/// pipeline step individually and is the Rust counterpart of Python/TS
/// step-level middleware (Issue #33 §2.2).
///
/// Hook semantics:
/// - [`Self::before_step`] runs before every step. Returning `Err` aborts the
///   pipeline immediately with the returned error wrapped in `PipelineStepError`.
/// - [`Self::after_step`] runs after a step succeeds. Errors propagate the same
///   way as `before_step`.
/// - [`Self::on_step_error`] runs when a step's `execute()` returns `Err` and
///   the step is **not** marked `ignore_errors`. Returning `Ok(Some(value))`
///   recovers — `ctx.output` is replaced with `value`, the failure is treated
///   as a successful continue, and `after_step` is NOT invoked for the recovered
///   step. Returning `Ok(None)` lets the original error propagate.
///
/// Multiple middlewares run in registration order during the before phase and
/// reverse order during the after phase, mirroring the global middleware
/// chain semantics.
#[async_trait]
pub trait StepMiddleware: Send + Sync {
    /// Called before each step's `execute()` method.
    async fn before_step(
        &self,
        _step_name: &str,
        _state: &PipelineState<'_>,
    ) -> Result<(), ModuleError> {
        Ok(())
    }

    /// Called after each step's `execute()` method completes successfully.
    /// `result` is a snapshot of `ctx.output` taken right after the step ran;
    /// when the step did not set an output it is `Value::Null`.
    async fn after_step(
        &self,
        _step_name: &str,
        _state: &PipelineState<'_>,
        _result: &serde_json::Value,
    ) -> Result<(), ModuleError> {
        Ok(())
    }

    /// Called when a step's `execute()` returns `Err`. Returning `Ok(Some(value))`
    /// recovers the failure: `ctx.output` is set to `value` and the pipeline
    /// continues. `Ok(None)` lets the original error propagate.
    async fn on_step_error(
        &self,
        _step_name: &str,
        _state: &PipelineState<'_>,
        _error: &ModuleError,
    ) -> Result<Option<serde_json::Value>, ModuleError> {
        Ok(None)
    }
}

// ---------------------------------------------------------------------------
// Step trait
// ---------------------------------------------------------------------------

/// A single unit of work in the execution pipeline.
///
/// Step implementations receive their configuration via constructor — the trait
/// only defines the `execute()` contract and metadata accessors.
#[async_trait]
pub trait Step: Send + Sync {
    /// Unique identifier within a strategy (e.g. "`acl_check`").
    fn name(&self) -> &str;

    /// AI-readable purpose description.
    fn description(&self) -> &str;

    /// Whether this step can be removed from the pipeline.
    /// Safety-critical steps return `false`.
    fn removable(&self) -> bool;

    /// Whether this step's implementation can be swapped.
    fn replaceable(&self) -> bool;

    /// Glob patterns for module IDs this step applies to. `None` = all.
    fn match_modules(&self) -> Option<&[String]> {
        None
    }

    /// `true` = step failure logs warning and continues. `false` = step failure aborts pipeline.
    fn ignore_errors(&self) -> bool {
        false
    }

    /// `true` = no side effects. Safe to run during `validate()` (`dry_run` mode).
    fn pure(&self) -> bool {
        false
    }

    /// Per-step timeout in milliseconds. `0` = no per-step timeout.
    fn timeout_ms(&self) -> u64 {
        0
    }

    /// `PipelineContext` fields this step reads (e.g. `["module", "context"]`). Advisory only.
    fn requires(&self) -> &[&str] {
        &[]
    }

    /// `PipelineContext` fields this step writes (e.g. `["output"]`). Advisory only.
    fn provides(&self) -> &[&str] {
        &[]
    }

    /// Execute the step, reading/writing shared [`PipelineContext`] state.
    async fn execute(&self, ctx: &mut PipelineContext) -> Result<StepResult, ModuleError>;
}

// ---------------------------------------------------------------------------
// StepResult
// ---------------------------------------------------------------------------

/// The outcome of a step execution, with AI-readable metadata.
///
/// `StepResult` only controls flow (continue / skip / abort) and provides
/// explanatory metadata — it does NOT carry data between steps.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    /// Flow control action: `"continue"`, `"skip_to"`, or `"abort"`.
    pub action: String,
    /// Target step name when `action` is `"skip_to"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_to: Option<String>,
    /// AI/human-readable explanation of the decision.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
    /// AI decision confidence (0.0–1.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    /// Suggested alternatives when the action is `"abort"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alternatives: Option<Vec<String>>,
}

impl Default for StepResult {
    fn default() -> Self {
        Self {
            action: "continue".into(),
            skip_to: None,
            explanation: None,
            confidence: None,
            alternatives: None,
        }
    }
}

impl StepResult {
    /// Create a result that continues to the next step.
    #[must_use]
    pub fn continue_step() -> Self {
        Self::default()
    }

    /// Create a result that aborts the pipeline with an explanation.
    #[must_use]
    pub fn abort(explanation: &str) -> Self {
        Self {
            action: "abort".into(),
            explanation: Some(explanation.to_string()),
            ..Default::default()
        }
    }

    /// Create a result that skips forward to the named step.
    #[must_use]
    pub fn skip_to(target: &str) -> Self {
        Self {
            action: "skip_to".into(),
            skip_to: Some(target.to_string()),
            ..Default::default()
        }
    }
}

// ---------------------------------------------------------------------------
// PipelineContext
// ---------------------------------------------------------------------------

/// Shared mutable state flowing through all pipeline steps.
///
/// Fields follow a two-tier model:
/// - **Tier 1** (direct fields): pipeline-essential data written by built-in steps.
/// - **Tier 2** (`context.data`): extension state written by middleware/custom steps
///   via `ContextKey`.
pub struct PipelineContext {
    /// Module being invoked.
    pub module_id: String,
    /// Original inputs (may be mutated by `middleware_before`).
    pub inputs: serde_json::Value,
    /// `APCore` execution context.
    pub context: Context<serde_json::Value>,

    // -- Resolved during pipeline (None until the responsible step runs) --
    /// Set by `module_lookup` step.
    pub module: Option<Arc<dyn Module>>,
    /// Set by `input_validation` step.
    pub validated_inputs: Option<serde_json::Value>,
    /// Set by `execute` step (non-streaming).
    pub output: Option<serde_json::Value>,
    /// Set by `output_validation` step.
    pub validated_output: Option<serde_json::Value>,

    // -- Pipeline v2 --
    /// `true` during `validate()`. `PipelineEngine` skips steps with `pure=false`.
    pub dry_run: bool,
    /// Passed through to `module_lookup` for version negotiation.
    pub version_hint: Option<String>,
    /// Tracks which middleware ran, enabling `on_error` recovery chain.
    pub executed_middlewares: Vec<usize>,

    // -- Executor resources (injected by Executor::call) --
    /// Module registry for lookups.
    pub registry: Option<Arc<Registry>>,
    /// Executor configuration (timeouts, call depth limits, etc.).
    pub config: Option<Arc<Config>>,
    /// Access control list, if configured.
    pub acl: Option<Arc<ACL>>,
    /// Approval handler, if configured.
    pub approval_handler: Option<Arc<dyn ApprovalHandler>>,
    /// Middleware manager for before/after chains.
    pub middleware_manager: Option<Arc<MiddlewareManager>>,

    // -- Metadata --
    /// Name of the strategy driving this execution.
    pub strategy_name: String,
    /// Accumulates step-level trace records.
    pub trace: PipelineTrace,
}

impl PipelineContext {
    /// Create a new `PipelineContext` for a module invocation.
    pub fn new(
        module_id: impl Into<String>,
        inputs: serde_json::Value,
        context: Context<serde_json::Value>,
        strategy_name: impl Into<String>,
    ) -> Self {
        let module_id = module_id.into();
        let strategy_name = strategy_name.into();
        Self {
            trace: PipelineTrace::new(module_id.clone(), strategy_name.clone()),
            module_id,
            inputs,
            context,
            module: None,
            validated_inputs: None,
            output: None,
            validated_output: None,
            dry_run: false,
            version_hint: None,
            executed_middlewares: vec![],
            registry: None,
            config: None,
            acl: None,
            approval_handler: None,
            middleware_manager: None,
            strategy_name,
        }
    }
}

// ---------------------------------------------------------------------------
// Trace types
// ---------------------------------------------------------------------------

/// A single step's execution record within a pipeline trace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepTrace {
    /// Step name.
    pub name: String,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: f64,
    /// The step's result (action, explanation, confidence).
    pub result: StepResult,
    /// Whether this step was skipped (e.g. via `skip_to`).
    pub skipped: bool,
    /// Whether this step is an AI decision point.
    pub decision_point: bool,
    /// Reason the step was skipped: "`no_match`", "`dry_run`", or "`error_ignored`".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
}

/// Complete execution record for a pipeline run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineTrace {
    /// Module that was invoked.
    pub module_id: String,
    /// Strategy that was used.
    pub strategy_name: String,
    /// Per-step records, in execution order.
    pub steps: Vec<StepTrace>,
    /// Total pipeline duration in milliseconds.
    pub total_duration_ms: f64,
    /// Whether the pipeline completed successfully.
    pub success: bool,
}

impl PipelineTrace {
    /// Create an empty trace for a new pipeline run.
    #[must_use]
    pub fn new(module_id: String, strategy_name: String) -> Self {
        Self {
            module_id,
            strategy_name,
            steps: Vec::new(),
            total_duration_ms: 0.0,
            success: false,
        }
    }
}

// ---------------------------------------------------------------------------
// PipelineState
// ---------------------------------------------------------------------------

/// Snapshot passed to a [`RunUntilPredicate`] after each pipeline step
/// completes. See `core-executor.md` §Pipeline Hardening §1.4.
///
/// Mirrors `apcore.pipeline.PipelineState` in Python.
pub struct PipelineState<'a> {
    /// Name of the step that just completed.
    pub step_name: &'a str,
    /// Output snapshots, keyed by step name. The value is a shallow copy of
    /// `ctx.output` taken right after the step ran (or `None` if the step did
    /// not set an output). Snapshots are append-only across the run.
    pub outputs: &'a HashMap<String, Option<serde_json::Value>>,
    /// The live pipeline context. Held by reference — predicates must not
    /// mutate state through it.
    pub context: &'a PipelineContext,
}

// ---------------------------------------------------------------------------
// StrategyInfo
// ---------------------------------------------------------------------------

/// AI-introspectable summary of an [`ExecutionStrategy`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyInfo {
    /// Strategy name.
    pub name: String,
    /// Number of steps in the strategy.
    pub step_count: usize,
    /// Ordered list of step names.
    pub step_names: Vec<String>,
    /// Auto-generated description from step descriptions.
    pub description: String,
}

impl std::fmt::Display for StrategyInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}-step pipeline: {}",
            self.step_count,
            self.step_names.join(" \u{2192} ")
        )
    }
}

// ---------------------------------------------------------------------------
// ExecutionStrategy
// ---------------------------------------------------------------------------

/// An ordered list of steps defining a complete execution pipeline.
///
/// Provides safe mutation methods that enforce removability / replaceability
/// constraints and step-name uniqueness.
///
/// Maintains an internal `HashMap<String, usize>` index from step name to
/// position so step lookups (used by `skip_to`, `configure_step`, and the
/// streaming `run_until_step` path) are O(1) per `core-executor.md`
/// §Pipeline Hardening §1.5. The index is rebuilt after every mutation.
pub struct ExecutionStrategy {
    name: String,
    steps: Vec<Box<dyn Step>>,
    name_to_idx: HashMap<String, usize>,
    step_middlewares: Vec<Arc<dyn StepMiddleware>>,
}

impl std::fmt::Debug for ExecutionStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecutionStrategy")
            .field("name", &self.name)
            .field("step_names", &self.step_names())
            .field("step_count", &self.steps.len())
            .field("name_to_idx_len", &self.name_to_idx.len())
            .field("step_middleware_count", &self.step_middlewares.len())
            .finish()
    }
}

/// No-op step used as a temporary placeholder by [`ExecutionStrategy::replace_with`].
struct PlaceholderStep;

#[async_trait]
impl Step for PlaceholderStep {
    fn name(&self) -> &'static str {
        "__placeholder__"
    }
    fn description(&self) -> &'static str {
        ""
    }
    fn removable(&self) -> bool {
        true
    }
    fn replaceable(&self) -> bool {
        true
    }
    async fn execute(&self, _ctx: &mut PipelineContext) -> Result<StepResult, ModuleError> {
        Ok(StepResult::continue_step())
    }
}

impl ExecutionStrategy {
    /// Create a new strategy with the given name and initial steps.
    ///
    /// Returns an error if any two steps share the same name.
    pub fn new(name: impl Into<String>, steps: Vec<Box<dyn Step>>) -> Result<Self, ModuleError> {
        let name = name.into();
        // Check for duplicate step names.
        let mut seen = std::collections::HashSet::new();
        for step in &steps {
            if !seen.insert(step.name().to_string()) {
                return Err(ModuleError::new(
                    ErrorCode::GeneralInvalidInput,
                    format!(
                        "Duplicate step name '{}' in strategy '{}'",
                        step.name(),
                        name,
                    ),
                ));
            }
        }
        let mut strategy = Self {
            name,
            steps,
            name_to_idx: HashMap::new(),
            step_middlewares: Vec::new(),
        };
        strategy.rebuild_index();
        // §2.1: fail-fast on unmet requires/provides at construction.
        strategy.validate_dependencies()?;
        Ok(strategy)
    }

    /// Rebuild the O(1) name→index map. Called after any structural mutation.
    fn rebuild_index(&mut self) {
        self.name_to_idx = self
            .steps
            .iter()
            .enumerate()
            .map(|(i, s)| (s.name().to_string(), i))
            .collect();
    }

    /// Read-only access to the step name→index map. Useful for tests asserting
    /// O(1) lookup compliance per §1.5.
    #[must_use]
    pub fn name_to_idx(&self) -> &HashMap<String, usize> {
        &self.name_to_idx
    }

    /// Validate `requires`/`provides` declarations across the strategy.
    ///
    /// Issue #33 §2.1: rather than logging a `tracing::warn!` and proceeding
    /// (the previous behaviour), an unmet `requires` reference now causes
    /// strategy construction to fail with [`ErrorCode::PipelineDependencyError`].
    /// This makes pipeline misconfiguration a startup-time error rather than
    /// a latent runtime hazard.
    fn validate_dependencies(&self) -> Result<(), ModuleError> {
        let mut provided = std::collections::HashSet::new();
        for step in &self.steps {
            for req in step.requires() {
                if !provided.contains(*req) {
                    return Err(ModuleError::new(
                        ErrorCode::PipelineDependencyError,
                        format!(
                            "Step '{}' in strategy '{}' requires '{}', but no preceding step \
                             provides it",
                            step.name(),
                            self.name,
                            req,
                        ),
                    ));
                }
            }
            for p in step.provides() {
                provided.insert(*p);
            }
        }
        Ok(())
    }

    /// Register a [`StepMiddleware`] for this strategy. Middlewares execute in
    /// registration order during the before phase and reverse order during the
    /// after phase. See [`StepMiddleware`] (Issue #33 §2.2).
    pub fn add_step_middleware(&mut self, mw: Arc<dyn StepMiddleware>) {
        self.step_middlewares.push(mw);
    }

    /// Read-only access to the registered step middlewares.
    #[must_use]
    pub fn step_middlewares(&self) -> &[Arc<dyn StepMiddleware>] {
        &self.step_middlewares
    }

    /// Strategy name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Rename this strategy.
    pub fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }

    /// Ordered list of step names.
    #[must_use]
    pub fn step_names(&self) -> Vec<String> {
        self.steps.iter().map(|s| s.name().to_string()).collect()
    }

    /// Read-only access to the step list.
    #[must_use]
    pub fn steps(&self) -> &[Box<dyn Step>] {
        &self.steps
    }

    /// Insert a step immediately after the named anchor.
    pub fn insert_after(&mut self, anchor: &str, step: Box<dyn Step>) -> Result<(), ModuleError> {
        self.validate_no_duplicate(step.name())?;
        let idx = self.find_step_index(anchor)?;
        self.steps.insert(idx + 1, step);
        self.rebuild_index();
        self.validate_dependencies()?;
        Ok(())
    }

    /// Insert a step immediately before the named anchor.
    pub fn insert_before(&mut self, anchor: &str, step: Box<dyn Step>) -> Result<(), ModuleError> {
        self.validate_no_duplicate(step.name())?;
        let idx = self.find_step_index(anchor)?;
        self.steps.insert(idx, step);
        self.rebuild_index();
        self.validate_dependencies()?;
        Ok(())
    }

    /// Remove a step by name. Fails if the step is not removable.
    pub fn remove(&mut self, step_name: &str) -> Result<(), ModuleError> {
        let idx = self.find_step_index(step_name)?;
        if !self.steps[idx].removable() {
            return Err(ModuleError::new(
                ErrorCode::GeneralInvalidInput,
                format!("Step '{step_name}' is not removable"),
            ));
        }
        self.steps.remove(idx);
        self.rebuild_index();
        Ok(())
    }

    /// Replace a step's implementation. Fails if the step is not replaceable.
    pub fn replace(&mut self, step_name: &str, new_step: Box<dyn Step>) -> Result<(), ModuleError> {
        let idx = self.find_step_index(step_name)?;
        if !self.steps[idx].replaceable() {
            return Err(ModuleError::new(
                ErrorCode::GeneralInvalidInput,
                format!("Step '{step_name}' is not replaceable"),
            ));
        }
        // Step name may differ from original; rebuild the index either way.
        self.steps[idx] = new_step;
        self.rebuild_index();
        Ok(())
    }

    /// Configure a step by replacing it in place — `core-executor.md`
    /// §Pipeline Hardening §1.2 (Replace Semantic).
    ///
    /// Calling this twice with the same `step_name` is idempotent: there is
    /// always exactly one step at the original position. Fails with
    /// [`ErrorCode::PipelineStepNotFound`] when the target does not exist.
    pub fn configure_step(
        &mut self,
        step_name: &str,
        new_step: Box<dyn Step>,
    ) -> Result<(), ModuleError> {
        let idx = self.name_to_idx.get(step_name).copied().ok_or_else(|| {
            ModuleError::new(
                ErrorCode::PipelineStepNotFound,
                format!("Pipeline step not found: '{step_name}'"),
            )
        })?;
        self.steps[idx] = new_step;
        self.rebuild_index();
        Ok(())
    }

    /// Replace a step by applying a wrapper function over its current value.
    ///
    /// Used by `build_strategy_from_config` to overlay YAML metadata
    /// (`match_modules`, `ignore_errors`, etc.) on built-in steps without
    /// losing the original step logic.
    pub fn replace_with<F>(&mut self, step_name: &str, wrapper: F) -> Result<(), ModuleError>
    where
        F: FnOnce(Box<dyn Step>) -> Box<dyn Step>,
    {
        let idx = self.find_step_index(step_name)?;
        let old = std::mem::replace(&mut self.steps[idx], Box::new(PlaceholderStep));
        self.steps[idx] = wrapper(old);
        self.rebuild_index();
        Ok(())
    }

    /// Build a [`StrategyInfo`] summary for AI introspection.
    #[must_use]
    pub fn info(&self) -> StrategyInfo {
        let step_names = self.step_names();
        let description = self
            .steps
            .iter()
            .map(|s| format!("{}: {}", s.name(), s.description()))
            .collect::<Vec<_>>()
            .join("; ");
        StrategyInfo {
            name: self.name.clone(),
            step_count: self.steps.len(),
            step_names,
            description,
        }
    }

    // -- helpers --

    fn find_step_index(&self, step_name: &str) -> Result<usize, ModuleError> {
        // O(1) via name_to_idx (§1.5).
        self.name_to_idx.get(step_name).copied().ok_or_else(|| {
            ModuleError::new(
                ErrorCode::GeneralInvalidInput,
                format!("Step '{}' not found in strategy '{}'", step_name, self.name),
            )
        })
    }

    fn validate_no_duplicate(&self, name: &str) -> Result<(), ModuleError> {
        if self.name_to_idx.contains_key(name) {
            return Err(ModuleError::new(
                ErrorCode::GeneralInvalidInput,
                format!(
                    "Step name '{}' already exists in strategy '{}'",
                    name, self.name,
                ),
            ));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// PipelineEngine
// ---------------------------------------------------------------------------

/// Executes an [`ExecutionStrategy`] against a [`PipelineContext`], returning
/// the final output and a complete execution trace.
pub struct PipelineEngine;

/// Optional knobs for [`PipelineEngine::run_with_options`]. All fields are
/// independent — set only what you need.
#[derive(Default)]
pub struct RunOptions {
    /// Halt execution **before** the step with this name (the step is not run
    /// and not recorded in the trace). Used by the streaming path to drive the
    /// shared engine up to the `execute` step. `None` runs to the end.
    pub stop_before_step: Option<String>,
    /// Predicate evaluated **after** every successful step completes. Returning
    /// `true` halts the pipeline; the current step's output is preserved.
    /// See `core-executor.md` §Pipeline Hardening §1.4.
    pub until: Option<RunUntilPredicate>,
}

impl RunOptions {
    /// Build options that stop before the step with the given name.
    #[must_use]
    pub fn stop_before(step_name: impl Into<String>) -> Self {
        Self {
            stop_before_step: Some(step_name.into()),
            until: None,
        }
    }

    /// Build options with a predicate-based termination condition.
    #[must_use]
    pub fn run_until<F>(predicate: F) -> Self
    where
        F: Fn(&PipelineState) -> bool + Send + Sync + 'static,
    {
        Self {
            stop_before_step: None,
            until: Some(Box::new(predicate)),
        }
    }
}

impl PipelineEngine {
    /// Run every step in `strategy` against `ctx`, respecting flow-control
    /// actions (`continue`, `skip_to`, `abort`).
    pub async fn run(
        strategy: &ExecutionStrategy,
        ctx: &mut PipelineContext,
    ) -> Result<(Option<serde_json::Value>, PipelineTrace), ModuleError> {
        Self::run_with_options(strategy, ctx, RunOptions::default()).await
    }

    /// Run with a predicate-based termination condition — `core-executor.md`
    /// §Pipeline Hardening §1.4.
    ///
    /// The predicate is evaluated **after** each step completes successfully.
    /// Returning `true` halts the pipeline and returns the accumulated output;
    /// returning `false` lets the pipeline proceed to the next step.
    pub async fn run_until<F>(
        strategy: &ExecutionStrategy,
        ctx: &mut PipelineContext,
        predicate: F,
    ) -> Result<(Option<serde_json::Value>, PipelineTrace), ModuleError>
    where
        F: Fn(&PipelineState) -> bool + Send + Sync + 'static,
    {
        Self::run_with_options(strategy, ctx, RunOptions::run_until(predicate)).await
    }

    /// Run every step in `strategy` against `ctx` UP TO (but not including)
    /// the named step. All step metadata — `match_modules`, `ignore_errors`,
    /// `timeout_ms`, `dry_run` purity filtering, `skip_to` flow control — is
    /// honored identically to [`Self::run`], so streaming and non-streaming
    /// paths never diverge on per-step semantics.
    ///
    /// Used by `Executor::stream` (with `stop_before_step = "execute"`) to
    /// prepare the pipeline without running the module itself, so the caller
    /// can then drive true chunk-by-chunk streaming.
    pub async fn run_until_step(
        strategy: &ExecutionStrategy,
        ctx: &mut PipelineContext,
        stop_before_step: &str,
    ) -> Result<(Option<serde_json::Value>, PipelineTrace), ModuleError> {
        Self::run_with_options(strategy, ctx, RunOptions::stop_before(stop_before_step)).await
    }

    /// Run with full control over termination. See [`RunOptions`].
    #[allow(clippy::too_many_lines)] // pipeline control loop is inherently stateful; splitting would reduce clarity
    pub async fn run_with_options(
        strategy: &ExecutionStrategy,
        ctx: &mut PipelineContext,
        options: RunOptions,
    ) -> Result<(Option<serde_json::Value>, PipelineTrace), ModuleError> {
        let pipeline_start = std::time::Instant::now();
        let steps = strategy.steps();
        let mut idx: usize = 0;
        // Snapshot of ctx.output keyed by step name, accumulated across the
        // run. Passed to run_until predicates as PipelineState::outputs.
        let mut step_outputs: HashMap<String, Option<serde_json::Value>> = HashMap::new();

        while idx < steps.len() {
            let step = &steps[idx];

            // Early exit for streaming / partial-pipeline callers.
            if let Some(stop_name) = options.stop_before_step.as_deref() {
                if step.name() == stop_name {
                    break;
                }
            }

            // Read declarations (trait defaults for backward compat)
            let step_match_modules = step.match_modules();
            let step_ignore_errors = step.ignore_errors();
            let step_pure = step.pure();
            let step_timeout_ms = step.timeout_ms();

            // (1) match_modules filter
            if let Some(patterns) = step_match_modules {
                let matched = patterns
                    .iter()
                    .any(|pattern| match_pattern(pattern, &ctx.module_id));
                if !matched {
                    ctx.trace.steps.push(StepTrace {
                        name: step.name().to_string(),
                        duration_ms: 0.0,
                        result: StepResult::continue_step(),
                        skipped: true,
                        decision_point: false,
                        skip_reason: Some("no_match".to_string()),
                    });
                    idx += 1;
                    continue;
                }
            }

            // (2) dry_run filter: skip steps with side effects
            if ctx.dry_run && !step_pure {
                ctx.trace.steps.push(StepTrace {
                    name: step.name().to_string(),
                    duration_ms: 0.0,
                    result: StepResult::continue_step(),
                    skipped: true,
                    decision_point: false,
                    skip_reason: Some("dry_run".to_string()),
                });
                idx += 1;
                continue;
            }

            // (3a) StepMiddleware: before_step hooks run in registration order.
            //      A failure here aborts the pipeline like any step error.
            let step_name_for_hooks = step.name().to_string();
            let middlewares = strategy.step_middlewares();
            let mut before_err: Option<ModuleError> = None;
            for mw in middlewares {
                let state = PipelineState {
                    step_name: &step_name_for_hooks,
                    outputs: &step_outputs,
                    context: ctx,
                };
                if let Err(e) = mw.before_step(&step_name_for_hooks, &state).await {
                    before_err = Some(e);
                    break;
                }
            }
            if let Some(err) = before_err {
                ctx.trace.steps.push(StepTrace {
                    name: step_name_for_hooks.clone(),
                    duration_ms: 0.0,
                    result: StepResult::abort(&err.to_string()),
                    skipped: false,
                    decision_point: false,
                    skip_reason: None,
                });
                ctx.trace.total_duration_ms = pipeline_start.elapsed().as_secs_f64() * 1000.0;
                return Err(ModuleError::pipeline_step_error(&step_name_for_hooks, &err));
            }

            // (3) Execute with per-step timeout
            let step_start = std::time::Instant::now();
            let exec_result = if step_timeout_ms > 0 {
                match tokio::time::timeout(
                    std::time::Duration::from_millis(step_timeout_ms),
                    step.execute(ctx),
                )
                .await
                {
                    Ok(r) => r,
                    Err(_elapsed) => Err(ModuleError::new(
                        ErrorCode::ModuleTimeout,
                        format!(
                            "Step '{}' timed out after {}ms",
                            step.name(),
                            step_timeout_ms
                        ),
                    )),
                }
            } else {
                step.execute(ctx).await
            };

            let duration_ms = step_start.elapsed().as_secs_f64() * 1000.0;

            let result = match exec_result {
                Ok(r) => r,
                Err(err) => {
                    // (3b) StepMiddleware: on_step_error hooks may recover by
                    // returning Some(value). The first middleware to return a
                    // recovery value wins; remaining middlewares are skipped.
                    let mut recovery: Option<serde_json::Value> = None;
                    let mut hook_err: Option<ModuleError> = None;
                    for mw in middlewares {
                        let state = PipelineState {
                            step_name: &step_name_for_hooks,
                            outputs: &step_outputs,
                            context: ctx,
                        };
                        match mw.on_step_error(&step_name_for_hooks, &state, &err).await {
                            Ok(Some(v)) => {
                                recovery = Some(v);
                                break;
                            }
                            Ok(None) => {}
                            Err(e) => {
                                hook_err = Some(e);
                                break;
                            }
                        }
                    }
                    if let Some(e) = hook_err {
                        ctx.trace.steps.push(StepTrace {
                            name: step_name_for_hooks.clone(),
                            duration_ms,
                            result: StepResult::abort(&e.to_string()),
                            skipped: false,
                            decision_point: false,
                            skip_reason: None,
                        });
                        ctx.trace.total_duration_ms =
                            pipeline_start.elapsed().as_secs_f64() * 1000.0;
                        return Err(ModuleError::pipeline_step_error(&step_name_for_hooks, &e));
                    }
                    if let Some(value) = recovery {
                        ctx.output = Some(value);
                        ctx.trace.steps.push(StepTrace {
                            name: step_name_for_hooks.clone(),
                            duration_ms,
                            result: StepResult {
                                action: "continue".into(),
                                explanation: Some(format!("recovered from: {err}")),
                                ..Default::default()
                            },
                            skipped: false,
                            decision_point: false,
                            skip_reason: Some("error_recovered".to_string()),
                        });
                        step_outputs.insert(step_name_for_hooks.clone(), ctx.output.clone());
                        idx += 1;
                        continue;
                    }

                    // (4) ignore_errors: log and continue (§1.1)
                    if step_ignore_errors {
                        tracing::warn!(
                            step = step.name(),
                            error = %err,
                            "Step failed (ignored)"
                        );
                        ctx.trace.steps.push(StepTrace {
                            name: step.name().to_string(),
                            duration_ms,
                            result: StepResult {
                                action: "continue".into(),
                                explanation: Some(err.to_string()),
                                ..Default::default()
                            },
                            skipped: false,
                            decision_point: false,
                            skip_reason: Some("error_ignored".to_string()),
                        });
                        idx += 1;
                        continue;
                    }
                    // Fail-fast (§1.1): record the abort in the trace and
                    // surface a `PipelineStepError` carrying the step name and
                    // original cause. Executor consumers (call/validate)
                    // unwrap this back to the original error before returning
                    // to user code.
                    ctx.trace.steps.push(StepTrace {
                        name: step.name().to_string(),
                        duration_ms,
                        result: StepResult::abort(&err.to_string()),
                        skipped: false,
                        decision_point: false,
                        skip_reason: None,
                    });
                    ctx.trace.total_duration_ms = pipeline_start.elapsed().as_secs_f64() * 1000.0;
                    return Err(ModuleError::pipeline_step_error(step.name(), &err));
                }
            };

            // (3c) StepMiddleware: after_step hooks run after a successful step.
            //      Snapshot of ctx.output is passed; null when step did not set output.
            let after_result_value = ctx.output.clone().unwrap_or(serde_json::Value::Null);
            let mut after_err: Option<ModuleError> = None;
            for mw in middlewares {
                let state = PipelineState {
                    step_name: &step_name_for_hooks,
                    outputs: &step_outputs,
                    context: ctx,
                };
                if let Err(e) = mw
                    .after_step(&step_name_for_hooks, &state, &after_result_value)
                    .await
                {
                    after_err = Some(e);
                    break;
                }
            }
            if let Some(err) = after_err {
                ctx.trace.steps.push(StepTrace {
                    name: step_name_for_hooks.clone(),
                    duration_ms,
                    result: StepResult::abort(&err.to_string()),
                    skipped: false,
                    decision_point: false,
                    skip_reason: None,
                });
                ctx.trace.total_duration_ms = pipeline_start.elapsed().as_secs_f64() * 1000.0;
                return Err(ModuleError::pipeline_step_error(&step_name_for_hooks, &err));
            }

            let action = result.action.clone();
            let skip_target = result.skip_to.clone();

            // (5) Record trace
            ctx.trace.steps.push(StepTrace {
                name: step.name().to_string(),
                duration_ms,
                result,
                skipped: false,
                decision_point: false,
                skip_reason: None,
            });

            // (6) Snapshot output for run_until predicates. `Value::clone` walks
            // the full JSON tree, so later in-place mutations of `ctx.output`
            // cannot alter the historical record. (Python's `dict(ctx.output)`
            // does only a one-level copy; the Rust snapshot is deeper but the
            // intent — a frozen copy — is the same.)
            let step_name_owned = step.name().to_string();
            step_outputs.insert(step_name_owned.clone(), ctx.output.clone());

            // (7) Handle abort / skip_to
            match action.as_str() {
                "continue" => {
                    // (8) run_until predicate (§1.4): evaluated after a clean
                    // continue. Stops further steps when it returns true; the
                    // pipeline reports success and returns the current output.
                    if let Some(ref predicate) = options.until {
                        let state = PipelineState {
                            step_name: &step_name_owned,
                            outputs: &step_outputs,
                            context: ctx,
                        };
                        if predicate(&state) {
                            ctx.trace.total_duration_ms =
                                pipeline_start.elapsed().as_secs_f64() * 1000.0;
                            ctx.trace.success = true;
                            return Ok((ctx.output.clone(), ctx.trace.clone()));
                        }
                    }
                    idx += 1;
                }
                "abort" => {
                    ctx.trace.total_duration_ms = pipeline_start.elapsed().as_secs_f64() * 1000.0;
                    ctx.trace.success = false;
                    return Ok((ctx.output.clone(), ctx.trace.clone()));
                }
                "skip_to" => {
                    let target = skip_target.as_deref().unwrap_or("");
                    // O(1) lookup via the strategy's name_to_idx map (§1.5).
                    // The lookup must reject same-position and backward
                    // targets explicitly to prevent infinite loops; the prior
                    // linear scan implicitly enforced this.
                    let target_idx = strategy
                        .name_to_idx()
                        .get(target)
                        .copied()
                        .filter(|t| *t > idx);
                    match target_idx {
                        Some(target_idx) => {
                            // Mark skipped steps in trace.
                            for step in steps.iter().take(target_idx).skip(idx + 1) {
                                ctx.trace.steps.push(StepTrace {
                                    name: step.name().to_string(),
                                    duration_ms: 0.0,
                                    result: StepResult::continue_step(),
                                    skipped: true,
                                    decision_point: false,
                                    skip_reason: None,
                                });
                            }
                            idx = target_idx;
                        }
                        None => {
                            return Err(ModuleError::new(
                                ErrorCode::StepNotFound,
                                format!(
                                    "skip_to target '{}' not found after step '{}'",
                                    target,
                                    step.name(),
                                ),
                            ));
                        }
                    }
                }
                other => {
                    return Err(ModuleError::new(
                        ErrorCode::GeneralInvalidInput,
                        format!("Unknown step action: '{other}'"),
                    ));
                }
            }
        }

        ctx.trace.total_duration_ms = pipeline_start.elapsed().as_secs_f64() * 1000.0;
        ctx.trace.success = true;
        Ok((ctx.output.clone(), ctx.trace.clone()))
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal step implementation for testing.
    struct FakeStep {
        name: String,
        description: String,
        removable: bool,
        replaceable: bool,
    }

    impl FakeStep {
        fn new(name: &str, removable: bool, replaceable: bool) -> Self {
            Self {
                name: name.to_string(),
                description: format!("Fake step: {name}"),
                removable,
                replaceable,
            }
        }

        fn boxed(name: &str, removable: bool, replaceable: bool) -> Box<dyn Step> {
            Box::new(Self::new(name, removable, replaceable))
        }
    }

    #[async_trait]
    impl Step for FakeStep {
        fn name(&self) -> &str {
            &self.name
        }
        fn description(&self) -> &str {
            &self.description
        }
        fn removable(&self) -> bool {
            self.removable
        }
        fn replaceable(&self) -> bool {
            self.replaceable
        }
        async fn execute(&self, _ctx: &mut PipelineContext) -> Result<StepResult, ModuleError> {
            Ok(StepResult::continue_step())
        }
    }

    #[test]
    fn test_step_result_continue() {
        let r = StepResult::continue_step();
        assert_eq!(r.action, "continue");
        assert!(r.skip_to.is_none());
        assert!(r.explanation.is_none());
    }

    #[test]
    fn test_step_result_abort() {
        let r = StepResult::abort("bad input");
        assert_eq!(r.action, "abort");
        assert_eq!(r.explanation.as_deref(), Some("bad input"));
    }

    #[test]
    fn test_step_result_skip_to() {
        let r = StepResult::skip_to("execute");
        assert_eq!(r.action, "skip_to");
        assert_eq!(r.skip_to.as_deref(), Some("execute"));
    }

    #[test]
    fn test_step_result_serde_round_trip() {
        let r = StepResult {
            action: "abort".into(),
            explanation: Some("denied".into()),
            confidence: Some(0.95),
            alternatives: Some(vec!["retry".into()]),
            ..Default::default()
        };
        let json = serde_json::to_string(&r).expect("serialize");
        let r2: StepResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(r2.action, "abort");
        assert_eq!(r2.confidence, Some(0.95));
    }

    #[test]
    fn test_strategy_new_rejects_duplicate_names() {
        let steps: Vec<Box<dyn Step>> = vec![
            FakeStep::boxed("a", true, true),
            FakeStep::boxed("a", true, true),
        ];
        let result = ExecutionStrategy::new("test", steps);
        assert!(result.is_err());
    }

    #[test]
    fn test_strategy_step_names() {
        let strategy = ExecutionStrategy::new(
            "default",
            vec![
                FakeStep::boxed("one", true, true),
                FakeStep::boxed("two", true, true),
                FakeStep::boxed("three", true, true),
            ],
        )
        .expect("create strategy");

        assert_eq!(strategy.step_names(), vec!["one", "two", "three"]);
    }

    #[test]
    fn test_strategy_insert_after() {
        let mut strategy = ExecutionStrategy::new(
            "s",
            vec![
                FakeStep::boxed("a", true, true),
                FakeStep::boxed("c", true, true),
            ],
        )
        .unwrap();

        strategy
            .insert_after("a", FakeStep::boxed("b", true, true))
            .unwrap();

        assert_eq!(strategy.step_names(), vec!["a", "b", "c"]);
    }

    #[test]
    fn test_strategy_insert_before() {
        let mut strategy = ExecutionStrategy::new(
            "s",
            vec![
                FakeStep::boxed("a", true, true),
                FakeStep::boxed("c", true, true),
            ],
        )
        .unwrap();

        strategy
            .insert_before("c", FakeStep::boxed("b", true, true))
            .unwrap();

        assert_eq!(strategy.step_names(), vec!["a", "b", "c"]);
    }

    #[test]
    fn test_strategy_insert_rejects_duplicate() {
        let mut strategy =
            ExecutionStrategy::new("s", vec![FakeStep::boxed("a", true, true)]).unwrap();

        let result = strategy.insert_after("a", FakeStep::boxed("a", true, true));
        assert!(result.is_err());
    }

    #[test]
    fn test_strategy_insert_rejects_unknown_anchor() {
        let mut strategy =
            ExecutionStrategy::new("s", vec![FakeStep::boxed("a", true, true)]).unwrap();

        let result = strategy.insert_after("missing", FakeStep::boxed("b", true, true));
        assert!(result.is_err());
    }

    #[test]
    fn test_strategy_remove() {
        let mut strategy = ExecutionStrategy::new(
            "s",
            vec![
                FakeStep::boxed("a", true, true),
                FakeStep::boxed("b", true, true),
            ],
        )
        .unwrap();

        strategy.remove("a").unwrap();
        assert_eq!(strategy.step_names(), vec!["b"]);
    }

    #[test]
    fn test_strategy_remove_non_removable() {
        let mut strategy =
            ExecutionStrategy::new("s", vec![FakeStep::boxed("core", false, false)]).unwrap();

        let result = strategy.remove("core");
        assert!(result.is_err());
    }

    #[test]
    fn test_strategy_replace() {
        let mut strategy =
            ExecutionStrategy::new("s", vec![FakeStep::boxed("a", true, true)]).unwrap();

        strategy
            .replace("a", FakeStep::boxed("a", true, true))
            .unwrap();

        assert_eq!(strategy.step_names(), vec!["a"]);
    }

    #[test]
    fn test_strategy_replace_non_replaceable() {
        let mut strategy =
            ExecutionStrategy::new("s", vec![FakeStep::boxed("a", true, false)]).unwrap();

        let result = strategy.replace("a", FakeStep::boxed("a", true, true));
        assert!(result.is_err());
    }

    #[test]
    fn test_strategy_info() {
        let strategy = ExecutionStrategy::new(
            "default",
            vec![
                FakeStep::boxed("one", true, true),
                FakeStep::boxed("two", false, true),
            ],
        )
        .unwrap();

        let info = strategy.info();
        assert_eq!(info.name, "default");
        assert_eq!(info.step_count, 2);
        assert_eq!(info.step_names, vec!["one", "two"]);
        assert!(info.description.contains("one"));
        assert!(info.description.contains("two"));
    }

    #[test]
    fn test_pipeline_trace_new() {
        let trace = PipelineTrace::new("my_module".into(), "default".into());
        assert_eq!(trace.module_id, "my_module");
        assert_eq!(trace.strategy_name, "default");
        assert!(trace.steps.is_empty());
        assert!(!trace.success);
    }

    #[test]
    fn test_pipeline_context_new() {
        let ctx_inner = Context::<serde_json::Value>::anonymous();
        let pctx = PipelineContext::new(
            "test_module",
            serde_json::json!({"key": "value"}),
            ctx_inner,
            "default",
        );
        assert_eq!(pctx.module_id, "test_module");
        assert_eq!(pctx.strategy_name, "default");
        assert!(pctx.module.is_none());
        assert!(pctx.validated_inputs.is_none());
        assert!(pctx.output.is_none());
        assert!(pctx.validated_output.is_none());
        assert_eq!(pctx.trace.module_id, "test_module");
    }

    // --- run_until / prepare_stream unification tests --------------------

    /// Step that records how many times it was invoked. Used to prove that
    /// `run_until` stops BEFORE the named step and that `match_modules`
    /// filtering runs on the streaming path exactly as on the non-streaming
    /// path.
    struct CountingStep {
        name: String,
        invocations: Arc<std::sync::atomic::AtomicUsize>,
        match_modules: Option<Vec<String>>,
    }

    #[async_trait]
    impl Step for CountingStep {
        fn name(&self) -> &str {
            &self.name
        }
        fn description(&self) -> &str {
            &self.name
        }
        fn removable(&self) -> bool {
            true
        }
        fn replaceable(&self) -> bool {
            true
        }
        fn match_modules(&self) -> Option<&[String]> {
            self.match_modules.as_deref()
        }
        async fn execute(&self, _ctx: &mut PipelineContext) -> Result<StepResult, ModuleError> {
            self.invocations
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(StepResult::continue_step())
        }
    }

    #[tokio::test]
    async fn run_until_stops_before_named_step() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let pre_count = Arc::new(AtomicUsize::new(0));
        let execute_count = Arc::new(AtomicUsize::new(0));
        let post_count = Arc::new(AtomicUsize::new(0));

        let steps: Vec<Box<dyn Step>> = vec![
            Box::new(CountingStep {
                name: "pre".into(),
                invocations: Arc::clone(&pre_count),
                match_modules: None,
            }),
            Box::new(CountingStep {
                name: "execute".into(),
                invocations: Arc::clone(&execute_count),
                match_modules: None,
            }),
            Box::new(CountingStep {
                name: "post".into(),
                invocations: Arc::clone(&post_count),
                match_modules: None,
            }),
        ];
        let strategy = ExecutionStrategy::new("test", steps).unwrap();
        let mut pctx = PipelineContext::new(
            "mod.x",
            serde_json::json!({}),
            Context::<serde_json::Value>::anonymous(),
            "test",
        );

        let (_, trace) = PipelineEngine::run_until_step(&strategy, &mut pctx, "execute")
            .await
            .unwrap();

        assert_eq!(pre_count.load(Ordering::SeqCst), 1, "'pre' runs once");
        assert_eq!(
            execute_count.load(Ordering::SeqCst),
            0,
            "'execute' must NOT run — run_until stops before it"
        );
        assert_eq!(post_count.load(Ordering::SeqCst), 0, "'post' must not run");
        assert!(trace.success);
    }

    #[tokio::test]
    async fn run_until_applies_match_modules_filtering() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        // Regression: previously `prepare_stream` had a bespoke loop that
        // skipped match_modules filtering, so a step declaring
        // `match_modules: ["api.*"]` would run even for `mod.other`. This
        // test confirms `run_until` inherits filtering from `run`.
        let filtered_count = Arc::new(AtomicUsize::new(0));

        let steps: Vec<Box<dyn Step>> = vec![
            Box::new(CountingStep {
                name: "filtered_step".into(),
                invocations: Arc::clone(&filtered_count),
                match_modules: Some(vec!["api.*".into()]),
            }),
            Box::new(CountingStep {
                name: "execute".into(),
                invocations: Arc::new(AtomicUsize::new(0)),
                match_modules: None,
            }),
        ];
        let strategy = ExecutionStrategy::new("test", steps).unwrap();
        let mut pctx = PipelineContext::new(
            "other.mod",
            serde_json::json!({}),
            Context::<serde_json::Value>::anonymous(),
            "test",
        );

        PipelineEngine::run_until_step(&strategy, &mut pctx, "execute")
            .await
            .unwrap();

        assert_eq!(
            filtered_count.load(Ordering::SeqCst),
            0,
            "step with match_modules=['api.*'] must be skipped when module_id='other.mod' \
             — confirms streaming and non-streaming paths share dispatch semantics"
        );
    }
}

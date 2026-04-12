// APCore Protocol — Execution pipeline types
// Spec reference: design-execution-pipeline.md (Sections 2, 3.3, 8.2)

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
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

// ---------------------------------------------------------------------------
// Step trait
// ---------------------------------------------------------------------------

/// A single unit of work in the execution pipeline.
///
/// Step implementations receive their configuration via constructor — the trait
/// only defines the `execute()` contract and metadata accessors.
#[async_trait]
pub trait Step: Send + Sync {
    /// Unique identifier within a strategy (e.g. "acl_check").
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

    /// `true` = no side effects. Safe to run during `validate()` (dry_run mode).
    fn pure(&self) -> bool {
        false
    }

    /// Per-step timeout in milliseconds. `0` = no per-step timeout.
    fn timeout_ms(&self) -> u64 {
        0
    }

    /// PipelineContext fields this step reads (e.g. `["module", "context"]`). Advisory only.
    fn requires(&self) -> &[&str] {
        &[]
    }

    /// PipelineContext fields this step writes (e.g. `["output"]`). Advisory only.
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
    pub fn continue_step() -> Self {
        Self::default()
    }

    /// Create a result that aborts the pipeline with an explanation.
    pub fn abort(explanation: &str) -> Self {
        Self {
            action: "abort".into(),
            explanation: Some(explanation.to_string()),
            ..Default::default()
        }
    }

    /// Create a result that skips forward to the named step.
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
    /// APCore execution context.
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
    /// `true` during `validate()`. PipelineEngine skips steps with `pure=false`.
    pub dry_run: bool,
    /// Passed through to module_lookup for version negotiation.
    pub version_hint: Option<String>,
    /// Tracks which middleware ran, enabling on_error recovery chain.
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
    /// Reason the step was skipped: "no_match", "dry_run", or "error_ignored".
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

// ---------------------------------------------------------------------------
// ExecutionStrategy
// ---------------------------------------------------------------------------

/// An ordered list of steps defining a complete execution pipeline.
///
/// Provides safe mutation methods that enforce removability / replaceability
/// constraints and step-name uniqueness.
pub struct ExecutionStrategy {
    name: String,
    steps: Vec<Box<dyn Step>>,
}

impl std::fmt::Debug for ExecutionStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecutionStrategy")
            .field("name", &self.name)
            .field("step_names", &self.step_names())
            .finish()
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
        let strategy = Self { name, steps };
        strategy.validate_dependencies();
        Ok(strategy)
    }

    /// Warn if any step's requires are not provided by a preceding step.
    fn validate_dependencies(&self) {
        let mut provided = std::collections::HashSet::new();
        for step in &self.steps {
            for req in step.requires() {
                if !provided.contains(*req) {
                    tracing::warn!(
                        step = step.name(),
                        requires = *req,
                        "Step requires '{}', but no preceding step provides it. \
                         This may cause runtime errors.",
                        req,
                    );
                }
            }
            for p in step.provides() {
                provided.insert(*p);
            }
        }
    }

    /// Strategy name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Rename this strategy.
    pub fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }

    /// Ordered list of step names.
    pub fn step_names(&self) -> Vec<String> {
        self.steps.iter().map(|s| s.name().to_string()).collect()
    }

    /// Read-only access to the step list.
    pub fn steps(&self) -> &[Box<dyn Step>] {
        &self.steps
    }

    /// Insert a step immediately after the named anchor.
    pub fn insert_after(&mut self, anchor: &str, step: Box<dyn Step>) -> Result<(), ModuleError> {
        self.validate_no_duplicate(step.name())?;
        let idx = self.find_step_index(anchor)?;
        self.steps.insert(idx + 1, step);
        self.validate_dependencies();
        Ok(())
    }

    /// Insert a step immediately before the named anchor.
    pub fn insert_before(&mut self, anchor: &str, step: Box<dyn Step>) -> Result<(), ModuleError> {
        self.validate_no_duplicate(step.name())?;
        let idx = self.find_step_index(anchor)?;
        self.steps.insert(idx, step);
        self.validate_dependencies();
        Ok(())
    }

    /// Remove a step by name. Fails if the step is not removable.
    pub fn remove(&mut self, step_name: &str) -> Result<(), ModuleError> {
        let idx = self.find_step_index(step_name)?;
        if !self.steps[idx].removable() {
            return Err(ModuleError::new(
                ErrorCode::GeneralInvalidInput,
                format!("Step '{}' is not removable", step_name),
            ));
        }
        self.steps.remove(idx);
        Ok(())
    }

    /// Replace a step's implementation. Fails if the step is not replaceable.
    pub fn replace(&mut self, step_name: &str, new_step: Box<dyn Step>) -> Result<(), ModuleError> {
        let idx = self.find_step_index(step_name)?;
        if !self.steps[idx].replaceable() {
            return Err(ModuleError::new(
                ErrorCode::GeneralInvalidInput,
                format!("Step '{}' is not replaceable", step_name),
            ));
        }
        self.steps[idx] = new_step;
        Ok(())
    }

    /// Build a [`StrategyInfo`] summary for AI introspection.
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
        self.steps
            .iter()
            .position(|s| s.name() == step_name)
            .ok_or_else(|| {
                ModuleError::new(
                    ErrorCode::GeneralInvalidInput,
                    format!("Step '{}' not found in strategy '{}'", step_name, self.name),
                )
            })
    }

    fn validate_no_duplicate(&self, name: &str) -> Result<(), ModuleError> {
        if self.steps.iter().any(|s| s.name() == name) {
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

impl PipelineEngine {
    /// Run every step in `strategy` against `ctx`, respecting flow-control
    /// actions (`continue`, `skip_to`, `abort`).
    pub async fn run(
        strategy: &ExecutionStrategy,
        ctx: &mut PipelineContext,
    ) -> Result<(Option<serde_json::Value>, PipelineTrace), ModuleError> {
        let pipeline_start = std::time::Instant::now();
        let steps = strategy.steps();
        let mut idx: usize = 0;

        while idx < steps.len() {
            let step = &steps[idx];

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
                    // (4) ignore_errors: log and continue
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
                    // Not ignored: record and raise
                    ctx.trace.steps.push(StepTrace {
                        name: step.name().to_string(),
                        duration_ms,
                        result: StepResult::abort(&err.to_string()),
                        skipped: false,
                        decision_point: false,
                        skip_reason: None,
                    });
                    ctx.trace.total_duration_ms = pipeline_start.elapsed().as_secs_f64() * 1000.0;
                    return Err(err);
                }
            };

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

            // (6) Handle abort / skip_to
            match action.as_str() {
                "continue" => {
                    idx += 1;
                }
                "abort" => {
                    ctx.trace.total_duration_ms = pipeline_start.elapsed().as_secs_f64() * 1000.0;
                    ctx.trace.success = false;
                    return Ok((ctx.output.clone(), ctx.trace.clone()));
                }
                "skip_to" => {
                    let target = skip_target.as_deref().unwrap_or("");
                    // Find the target step by name starting after current position.
                    let found = steps
                        .iter()
                        .enumerate()
                        .position(|(i, s)| i > idx && s.name() == target);
                    match found {
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
                                ErrorCode::GeneralInvalidInput,
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
                        format!("Unknown step action: '{}'", other),
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
                description: format!("Fake step: {}", name),
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
}

// APCore Protocol — Built-in execution pipeline steps
// Spec reference: design-execution-pipeline.md (Section 3)

use async_trait::async_trait;

use crate::errors::ModuleError;
use crate::pipeline::{ExecutionStrategy, PipelineContext, Step, StepResult};

// Macro to reduce boilerplate -- each step has name, description, removable, replaceable
macro_rules! builtin_step {
    ($name:ident, $step_name:expr, $desc:expr, removable=$rm:expr, replaceable=$rp:expr) => {
        pub struct $name;

        #[async_trait]
        impl Step for $name {
            fn name(&self) -> &str {
                $step_name
            }
            fn description(&self) -> &str {
                $desc
            }
            fn removable(&self) -> bool {
                $rm
            }
            fn replaceable(&self) -> bool {
                $rp
            }
            async fn execute(&self, _ctx: &mut PipelineContext) -> Result<StepResult, ModuleError> {
                Ok(StepResult::continue_step())
            }
        }
    };
}

builtin_step!(
    BuiltinContextCreation,
    "context_creation",
    "Create or inherit execution context",
    removable = false,
    replaceable = false
);
builtin_step!(
    BuiltinSafetyCheck,
    "safety_check",
    "Validate call depth and module repeat limits",
    removable = true,
    replaceable = true
);
builtin_step!(
    BuiltinModuleLookup,
    "module_lookup",
    "Resolve module from registry by ID",
    removable = false,
    replaceable = false
);
builtin_step!(
    BuiltinACLCheck,
    "acl_check",
    "Enforce access control rules",
    removable = true,
    replaceable = true
);
builtin_step!(
    BuiltinApprovalGate,
    "approval_gate",
    "Handle human or AI approval flow",
    removable = true,
    replaceable = true
);
builtin_step!(
    BuiltinInputValidation,
    "input_validation",
    "Validate inputs against schema",
    removable = true,
    replaceable = true
);
builtin_step!(
    BuiltinMiddlewareBefore,
    "middleware_before",
    "Execute before-middleware chain",
    removable = true,
    replaceable = false
);
builtin_step!(
    BuiltinExecute,
    "execute",
    "Invoke the module with timeout",
    removable = false,
    replaceable = true
);
builtin_step!(
    BuiltinOutputValidation,
    "output_validation",
    "Validate outputs against schema",
    removable = true,
    replaceable = true
);
builtin_step!(
    BuiltinMiddlewareAfter,
    "middleware_after",
    "Execute after-middleware chain",
    removable = true,
    replaceable = false
);
builtin_step!(
    BuiltinReturnResult,
    "return_result",
    "Finalize and return output",
    removable = false,
    replaceable = false
);

/// Build the standard 11-step execution strategy.
pub fn build_standard_strategy() -> ExecutionStrategy {
    // INVARIANT: all step names are unique literals, so `new()` cannot fail.
    ExecutionStrategy::new(
        "standard",
        vec![
            Box::new(BuiltinContextCreation),
            Box::new(BuiltinSafetyCheck),
            Box::new(BuiltinModuleLookup),
            Box::new(BuiltinACLCheck),
            Box::new(BuiltinApprovalGate),
            Box::new(BuiltinInputValidation),
            Box::new(BuiltinMiddlewareBefore),
            Box::new(BuiltinExecute),
            Box::new(BuiltinOutputValidation),
            Box::new(BuiltinMiddlewareAfter),
            Box::new(BuiltinReturnResult),
        ],
    )
    .expect("standard strategy should have unique step names")
}

/// Build the internal strategy (standard minus acl_check and approval_gate).
pub fn build_internal_strategy() -> ExecutionStrategy {
    let mut s = build_standard_strategy();
    s.set_name("internal");
    s.remove("acl_check").ok();
    s.remove("approval_gate").ok();
    s
}

/// Build the testing strategy (standard minus acl_check, approval_gate, and safety_check).
pub fn build_testing_strategy() -> ExecutionStrategy {
    let mut s = build_standard_strategy();
    s.set_name("testing");
    s.remove("acl_check").ok();
    s.remove("approval_gate").ok();
    s.remove("safety_check").ok();
    s
}

/// Build the performance strategy (standard minus middleware_before and middleware_after).
pub fn build_performance_strategy() -> ExecutionStrategy {
    let mut s = build_standard_strategy();
    s.set_name("performance");
    s.remove("middleware_before").ok();
    s.remove("middleware_after").ok();
    s
}

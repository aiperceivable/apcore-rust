// APCore Protocol — Helper utilities
// Spec reference: Pattern matching, call chain guards, error propagation

use crate::context::Context;
use crate::errors::ModuleError;

/// Match a string against a glob-like pattern (supports * and ?).
pub fn match_pattern(pattern: &str, value: &str) -> bool {
    // TODO: Implement — use regex crate for pattern matching
    todo!()
}

/// Guard against call depth and circular call violations.
pub fn guard_call_chain(
    ctx: &Context<serde_json::Value>,
    module_name: &str,
    max_depth: u32,
) -> Result<(), ModuleError> {
    // TODO: Implement
    todo!()
}

/// Propagate an error with additional context.
pub fn propagate_error(error: ModuleError, context: &str) -> ModuleError {
    // TODO: Implement — wrap error with context info
    todo!()
}

/// Normalize a module name or identifier to canonical form.
pub fn normalize_to_canonical_id(name: &str) -> String {
    // TODO: Implement — lowercase, trim, replace special chars
    todo!()
}

/// Calculate the specificity of a pattern for ACL rule ordering.
pub fn calculate_specificity(pattern: &str) -> u32 {
    // TODO: Implement — more specific patterns get higher scores
    todo!()
}

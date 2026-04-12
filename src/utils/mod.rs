// APCore Protocol — Utility functions
// Spec reference: Shared helpers and guard utilities

pub mod error_propagation;
pub mod helpers;

pub use error_propagation::{propagate_error, propagate_module_error};
pub use helpers::{
    calculate_specificity, guard_call_chain, guard_call_chain_with_repeat, match_pattern,
    normalize_to_canonical_id, DEFAULT_MAX_CALL_DEPTH, DEFAULT_MAX_MODULE_REPEAT,
};

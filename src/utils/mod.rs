// APCore Protocol — Utility functions
// Spec reference: Shared helpers and guard utilities

pub mod helpers;

pub use helpers::{
    calculate_specificity, guard_call_chain, guard_call_chain_with_repeat, match_pattern,
    normalize_to_canonical_id,
};

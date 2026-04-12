// APCore Protocol — ID conflict detection (Algorithm A03)
// Spec reference: Module ID conflict checks — duplicate, reserved, case collision

use std::collections::{HashMap, HashSet};

/// Result of an ID conflict check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictResult {
    /// One of: "duplicate_id", "reserved_word", "case_collision".
    pub conflict_type: String,
    /// "error" or "warning".
    pub severity: String,
    /// Human-readable conflict description.
    pub message: String,
}

/// Check if a new module ID conflicts with existing IDs or reserved words (Algorithm A03).
///
/// Steps:
///   1. Exact duplicate detection.
///   2. Reserved word detection (per segment).
///   3. Case collision detection.
///
/// Returns `Some(ConflictResult)` if a conflict is found, `None` if the ID is safe.
///
/// When `lowercase_map` is provided, case collision lookup is O(1).
/// Otherwise it falls back to an O(n) scan of `existing_ids`.
///
/// Aligned with `apcore-python.detect_id_conflicts` and
/// `apcore-typescript.detectIdConflicts`.
#[allow(clippy::implicit_hasher)] // public API: callers always use the default hasher
pub fn detect_id_conflicts(
    new_id: &str,
    existing_ids: &HashSet<String>,
    reserved_words: &[&str],
    lowercase_map: Option<&HashMap<String, String>>,
) -> Option<ConflictResult> {
    // Step 1: Exact duplicate
    if existing_ids.contains(new_id) {
        return Some(ConflictResult {
            conflict_type: "duplicate_id".to_string(),
            severity: "error".to_string(),
            message: format!("Module ID '{new_id}' is already registered"),
        });
    }

    // Step 2: Reserved word check (per segment)
    for segment in new_id.split('.') {
        if reserved_words.contains(&segment) {
            return Some(ConflictResult {
                conflict_type: "reserved_word".to_string(),
                severity: "error".to_string(),
                message: format!("Module ID '{new_id}' contains reserved word '{segment}'"),
            });
        }
    }

    // Step 3: Case collision
    let normalized_new = new_id.to_lowercase();
    if let Some(lc_map) = lowercase_map {
        if let Some(existing) = lc_map.get(&normalized_new) {
            if existing != new_id {
                return Some(ConflictResult {
                    conflict_type: "case_collision".to_string(),
                    severity: "warning".to_string(),
                    message: format!(
                        "Module ID '{new_id}' has a case collision with existing '{existing}'"
                    ),
                });
            }
        }
    } else {
        for existing_id in existing_ids {
            if existing_id.to_lowercase() == normalized_new && existing_id != new_id {
                return Some(ConflictResult {
                    conflict_type: "case_collision".to_string(),
                    severity: "warning".to_string(),
                    message: format!(
                        "Module ID '{new_id}' has a case collision with existing '{existing_id}'"
                    ),
                });
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_conflict() {
        let existing: HashSet<String> = ["foo.bar".to_string()].into_iter().collect();
        let reserved = &["system", "internal"];
        assert!(detect_id_conflicts("baz.qux", &existing, reserved, None).is_none());
    }

    #[test]
    fn test_duplicate_id() {
        let existing: HashSet<String> = ["foo.bar".to_string()].into_iter().collect();
        let result = detect_id_conflicts("foo.bar", &existing, &[], None).unwrap();
        assert_eq!(result.conflict_type, "duplicate_id");
        assert_eq!(result.severity, "error");
    }

    #[test]
    fn test_reserved_word() {
        let existing: HashSet<String> = HashSet::new();
        let reserved = &["system", "internal"];
        let result = detect_id_conflicts("system.foo", &existing, reserved, None).unwrap();
        assert_eq!(result.conflict_type, "reserved_word");
        assert_eq!(result.severity, "error");
    }

    #[test]
    fn test_case_collision_without_map() {
        let existing: HashSet<String> = ["Foo.Bar".to_string()].into_iter().collect();
        let result = detect_id_conflicts("foo.bar", &existing, &[], None).unwrap();
        assert_eq!(result.conflict_type, "case_collision");
        assert_eq!(result.severity, "warning");
    }

    #[test]
    fn test_case_collision_with_map() {
        let existing: HashSet<String> = ["Foo.Bar".to_string()].into_iter().collect();
        let mut lc_map = HashMap::new();
        lc_map.insert("foo.bar".to_string(), "Foo.Bar".to_string());
        let result = detect_id_conflicts("foo.bar", &existing, &[], Some(&lc_map)).unwrap();
        assert_eq!(result.conflict_type, "case_collision");
    }
}

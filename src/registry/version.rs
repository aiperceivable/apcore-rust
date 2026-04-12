// APCore Protocol — Version handling for registered modules (F18)
// Spec reference: Semver utilities and versioned storage for module version negotiation

use parking_lot::RwLock;
use std::collections::HashMap;

/// Parse a version string into a `(major, minor, patch)` tuple.
///
/// Supports full semver (`1.2.3`), major.minor (`1.2`), and major-only (`1`).
///
/// Aligned with `apcore-python.parse_semver` and
/// `apcore-typescript.parseSemver`.
pub fn parse_semver(version: &str) -> (u64, u64, u64) {
    let trimmed = version.trim();
    let mut parts = trimmed.splitn(3, '.');

    let major = parts.next().and_then(parse_numeric_prefix).unwrap_or(0);
    let minor = parts.next().and_then(parse_numeric_prefix).unwrap_or(0);
    let patch = parts.next().and_then(parse_numeric_prefix).unwrap_or(0);

    (major, minor, patch)
}

/// Parse the leading numeric portion of a string (handles pre-release suffixes like "3-beta").
fn parse_numeric_prefix(s: &str) -> Option<u64> {
    let numeric: String = s.chars().take_while(char::is_ascii_digit).collect();
    if numeric.is_empty() {
        None
    } else {
        numeric.parse().ok()
    }
}

/// Check if a version string satisfies a single constraint like `>=1.0.0`, `<2.0.0`, or `1.0.0` (exact).
fn check_single_constraint(version_tuple: (u64, u64, u64), constraint: &str) -> bool {
    let constraint = constraint.trim();
    if constraint.is_empty() {
        return false;
    }

    let (op, target_str) = if let Some(rest) = constraint.strip_prefix(">=") {
        (">=", rest)
    } else if let Some(rest) = constraint.strip_prefix("<=") {
        ("<=", rest)
    } else if let Some(rest) = constraint.strip_prefix('>') {
        (">", rest)
    } else if let Some(rest) = constraint.strip_prefix('<') {
        ("<", rest)
    } else if let Some(rest) = constraint.strip_prefix('=') {
        ("=", rest)
    } else {
        ("=", constraint)
    };

    let target = parse_semver(target_str);
    let parts: Vec<&str> = target_str.trim().split('.').collect();

    // Partial match for exact comparisons
    if op == "=" {
        if parts.len() == 1 {
            return version_tuple.0 == target.0;
        }
        if parts.len() == 2 {
            return version_tuple.0 == target.0 && version_tuple.1 == target.1;
        }
        return version_tuple == target;
    }

    match op {
        ">=" => version_tuple >= target,
        ">" => version_tuple > target,
        "<=" => version_tuple <= target,
        "<" => version_tuple < target,
        _ => false,
    }
}

/// Check if a version string satisfies a version hint.
///
/// The hint can be:
/// - An exact version: `"1.0.0"`
/// - A partial version: `"1"` (matches major 1.x.x)
/// - A constraint: `">=1.0.0"`, `"<2.0.0"`
/// - A comma-separated set of constraints: `">=1.0.0,<2.0.0"`
///
/// Aligned with `apcore-python.matches_version_hint` and
/// `apcore-typescript.matchesVersionHint`.
pub fn matches_version_hint(version: &str, hint: &str) -> bool {
    let version_tuple = parse_semver(version);
    hint.split(',')
        .all(|c| check_single_constraint(version_tuple, c.trim()))
}

/// Select the best matching version from a list.
///
/// If `version_hint` is `None`, returns the latest (highest) version.
/// If `version_hint` is given, returns the highest version that matches.
/// Returns `None` if no version matches.
///
/// Aligned with `apcore-python.select_best_version` and
/// `apcore-typescript.selectBestVersion`.
pub fn select_best_version(versions: &[String], version_hint: Option<&str>) -> Option<String> {
    if versions.is_empty() {
        return None;
    }

    let mut sorted: Vec<&String> = versions.iter().collect();
    sorted.sort_by_key(|a| parse_semver(a));

    match version_hint {
        None => sorted.last().map(|v| (*v).clone()),
        Some(hint) => sorted
            .iter()
            .rev()
            .find(|v| matches_version_hint(v, hint))
            .map(|v| (*v).clone()),
    }
}

/// Thread-safe storage for multiple versions of items keyed by ID.
///
/// Stores items as `HashMap<module_id, HashMap<version, T>>`.
///
/// Uses `parking_lot::RwLock` for consistency with the rest of the registry.
///
/// Aligned with `apcore-python.VersionedStore` and
/// `apcore-typescript.VersionedStore`.
pub struct VersionedStore<T> {
    data: RwLock<HashMap<String, HashMap<String, T>>>,
}

impl<T: Clone> VersionedStore<T> {
    /// Create a new empty versioned store.
    pub fn new() -> Self {
        Self {
            data: RwLock::new(HashMap::new()),
        }
    }

    /// Add an item for a given module_id and version.
    pub fn add(&self, module_id: &str, version: &str, item: T) {
        let mut data = self.data.write();
        data.entry(module_id.to_string())
            .or_default()
            .insert(version.to_string(), item);
    }

    /// Get a specific version of an item. Returns `None` if not found.
    pub fn get(&self, module_id: &str, version: &str) -> Option<T> {
        let data = self.data.read();
        data.get(module_id)
            .and_then(|versions| versions.get(version))
            .cloned()
    }

    /// Get the latest (highest semver) version of an item.
    pub fn get_latest(&self, module_id: &str) -> Option<T> {
        let data = self.data.read();
        let versions = data.get(module_id)?;
        let keys: Vec<String> = versions.keys().cloned().collect();
        let best = select_best_version(&keys, None)?;
        versions.get(&best).cloned()
    }

    /// Resolve a module by ID and optional version hint.
    pub fn resolve(&self, module_id: &str, version_hint: Option<&str>) -> Option<T> {
        let data = self.data.read();
        let versions = data.get(module_id)?;
        let keys: Vec<String> = versions.keys().cloned().collect();
        let best = select_best_version(&keys, version_hint)?;
        versions.get(&best).cloned()
    }

    /// List all registered versions for a module_id, sorted by semver.
    pub fn list_versions(&self, module_id: &str) -> Vec<String> {
        let data = self.data.read();
        match data.get(module_id) {
            Some(versions) => {
                let mut keys: Vec<String> = versions.keys().cloned().collect();
                keys.sort_by_key(|a| parse_semver(a));
                keys
            }
            None => Vec::new(),
        }
    }

    /// List all unique module IDs.
    pub fn list_ids(&self) -> Vec<String> {
        let data = self.data.read();
        data.keys().cloned().collect()
    }

    /// Remove a specific version. Returns the removed item or `None`.
    pub fn remove(&self, module_id: &str, version: &str) -> Option<T> {
        let mut data = self.data.write();
        let versions = data.get_mut(module_id)?;
        let item = versions.remove(version);
        if versions.is_empty() {
            data.remove(module_id);
        }
        item
    }

    /// Remove all versions for a module_id. Returns removed versions.
    pub fn remove_all(&self, module_id: &str) -> HashMap<String, T> {
        let mut data = self.data.write();
        data.remove(module_id).unwrap_or_default()
    }

    /// Check if any version of a module_id is registered.
    pub fn has(&self, module_id: &str) -> bool {
        let data = self.data.read();
        data.get(module_id).is_some_and(|v| !v.is_empty())
    }

    /// Check if a specific version is registered.
    pub fn has_version(&self, module_id: &str, version: &str) -> bool {
        let data = self.data.read();
        data.get(module_id).is_some_and(|v| v.contains_key(version))
    }
}

impl<T: Clone> Default for VersionedStore<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_semver_full() {
        assert_eq!(parse_semver("1.2.3"), (1, 2, 3));
    }

    #[test]
    fn test_parse_semver_partial() {
        assert_eq!(parse_semver("1.2"), (1, 2, 0));
        assert_eq!(parse_semver("1"), (1, 0, 0));
    }

    #[test]
    fn test_parse_semver_invalid() {
        assert_eq!(parse_semver("abc"), (0, 0, 0));
    }

    #[test]
    fn test_matches_version_hint_exact() {
        assert!(matches_version_hint("1.2.3", "1.2.3"));
        assert!(!matches_version_hint("1.2.4", "1.2.3"));
    }

    #[test]
    fn test_matches_version_hint_partial() {
        assert!(matches_version_hint("1.5.0", "1"));
        assert!(!matches_version_hint("2.0.0", "1"));
    }

    #[test]
    fn test_matches_version_hint_range() {
        assert!(matches_version_hint("1.5.0", ">=1.0.0,<2.0.0"));
        assert!(!matches_version_hint("2.0.0", ">=1.0.0,<2.0.0"));
        assert!(!matches_version_hint("0.9.0", ">=1.0.0,<2.0.0"));
    }

    #[test]
    fn test_select_best_version_latest() {
        let versions = vec![
            "1.0.0".to_string(),
            "2.0.0".to_string(),
            "1.5.0".to_string(),
        ];
        assert_eq!(
            select_best_version(&versions, None),
            Some("2.0.0".to_string())
        );
    }

    #[test]
    fn test_select_best_version_with_hint() {
        let versions = vec![
            "1.0.0".to_string(),
            "1.5.0".to_string(),
            "2.0.0".to_string(),
        ];
        assert_eq!(
            select_best_version(&versions, Some(">=1.0.0,<2.0.0")),
            Some("1.5.0".to_string())
        );
    }

    #[test]
    fn test_select_best_version_no_match() {
        let versions = vec!["1.0.0".to_string()];
        assert_eq!(select_best_version(&versions, Some(">=2.0.0")), None);
    }

    #[test]
    fn test_select_best_version_empty() {
        let versions: Vec<String> = vec![];
        assert_eq!(select_best_version(&versions, None), None);
    }

    #[test]
    fn test_versioned_store_basic() {
        let store: VersionedStore<String> = VersionedStore::new();
        store.add("foo", "1.0.0", "v1".to_string());
        store.add("foo", "2.0.0", "v2".to_string());

        assert_eq!(store.get("foo", "1.0.0"), Some("v1".to_string()));
        assert_eq!(store.get_latest("foo"), Some("v2".to_string()));
        assert!(store.has("foo"));
        assert!(store.has_version("foo", "1.0.0"));
        assert!(!store.has("bar"));
    }

    #[test]
    fn test_versioned_store_resolve() {
        let store: VersionedStore<String> = VersionedStore::new();
        store.add("foo", "1.0.0", "v1".to_string());
        store.add("foo", "1.5.0", "v15".to_string());
        store.add("foo", "2.0.0", "v2".to_string());

        assert_eq!(
            store.resolve("foo", Some(">=1.0.0,<2.0.0")),
            Some("v15".to_string())
        );
        assert_eq!(store.resolve("foo", None), Some("v2".to_string()));
    }

    #[test]
    fn test_versioned_store_remove() {
        let store: VersionedStore<String> = VersionedStore::new();
        store.add("foo", "1.0.0", "v1".to_string());
        store.add("foo", "2.0.0", "v2".to_string());

        assert_eq!(store.remove("foo", "1.0.0"), Some("v1".to_string()));
        assert!(!store.has_version("foo", "1.0.0"));
        assert!(store.has("foo"));

        let removed = store.remove_all("foo");
        assert_eq!(removed.len(), 1);
        assert!(!store.has("foo"));
    }

    #[test]
    fn test_versioned_store_list() {
        let store: VersionedStore<String> = VersionedStore::new();
        store.add("foo", "2.0.0", "v2".to_string());
        store.add("foo", "1.0.0", "v1".to_string());
        store.add("bar", "1.0.0", "bv1".to_string());

        let versions = store.list_versions("foo");
        assert_eq!(versions, vec!["1.0.0", "2.0.0"]);

        let mut ids = store.list_ids();
        ids.sort();
        assert_eq!(ids, vec!["bar", "foo"]);
    }
}

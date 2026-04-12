// APCore Protocol — Dependency resolution via Kahn's topological sort
// Spec reference: Module dependency ordering

use std::collections::{HashMap, HashSet, VecDeque};

use crate::errors::{ErrorCode, ModuleError};
use crate::registry::types::DepInfo;

/// Resolve module load order using Kahn's topological sort.
///
/// Modules with dependencies are loaded after their dependencies.
/// Optional dependencies that are missing are silently skipped.
///
/// Aligned with `apcore-python.resolve_dependencies` and
/// `apcore-typescript.resolveDependencies`.
#[allow(clippy::implicit_hasher)] // public API: callers always use the default hasher
pub fn resolve_dependencies(
    modules: &[(String, Vec<DepInfo>)],
    known_ids: Option<&HashSet<String>>,
) -> Result<Vec<String>, ModuleError> {
    if modules.is_empty() {
        return Ok(Vec::new());
    }

    let derived_ids: HashSet<String> = modules.iter().map(|(id, _)| id.clone()).collect();
    let effective_ids = known_ids.unwrap_or(&derived_ids);

    // Build graph and in-degree
    let mut graph: HashMap<String, Vec<String>> = HashMap::new();
    let mut in_degree: HashMap<String, usize> = HashMap::new();

    for (mod_id, _) in modules {
        in_degree.entry(mod_id.clone()).or_insert(0);
    }

    for (module_id, deps) in modules {
        for dep in deps {
            if !effective_ids.contains(&dep.module_id) {
                if dep.optional {
                    tracing::warn!(
                        "Optional dependency '{}' for module '{}' not found, skipping",
                        dep.module_id,
                        module_id
                    );
                    continue;
                }
                return Err(ModuleError::new(
                    ErrorCode::ModuleLoadError,
                    format!(
                        "Module '{}': required dependency '{}' not found",
                        module_id, dep.module_id
                    ),
                ));
            }
            graph
                .entry(dep.module_id.clone())
                .or_default()
                .push(module_id.clone());
            *in_degree.entry(module_id.clone()).or_insert(0) += 1;
        }
    }

    // Initialize queue with zero-in-degree nodes (sorted for determinism)
    let mut zero_degree: Vec<String> = in_degree
        .iter()
        .filter(|(_, &deg)| deg == 0)
        .map(|(id, _)| id.clone())
        .collect();
    zero_degree.sort();
    let mut queue: VecDeque<String> = zero_degree.into_iter().collect();

    let mut load_order: Vec<String> = Vec::new();
    while let Some(mod_id) = queue.pop_front() {
        load_order.push(mod_id.clone());
        if let Some(dependents) = graph.get(&mod_id) {
            let mut sorted_deps = dependents.clone();
            sorted_deps.sort();
            for dependent in sorted_deps {
                if let Some(deg) = in_degree.get_mut(&dependent) {
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push_back(dependent);
                    }
                }
            }
        }
    }

    // Check for cycles
    if load_order.len() < modules.len() {
        let ordered_set: HashSet<&String> = load_order.iter().collect();
        let remaining: HashSet<String> = modules
            .iter()
            .filter(|(id, _)| !ordered_set.contains(id))
            .map(|(id, _)| id.clone())
            .collect();
        let cycle_path = extract_cycle(modules, &remaining);
        return Err(ModuleError::new(
            ErrorCode::CircularDependency,
            format!("Circular dependency detected: {}", cycle_path.join(" -> ")),
        ));
    }

    Ok(load_order)
}

/// Extract a cycle path from the remaining unprocessed modules.
fn extract_cycle(modules: &[(String, Vec<DepInfo>)], remaining: &HashSet<String>) -> Vec<String> {
    let mut dep_map: HashMap<&str, Vec<&str>> = HashMap::new();
    for (mod_id, deps) in modules {
        if remaining.contains(mod_id) {
            let edges: Vec<&str> = deps
                .iter()
                .filter(|d| remaining.contains(&d.module_id))
                .map(|d| d.module_id.as_str())
                .collect();
            dep_map.insert(mod_id.as_str(), edges);
        }
    }

    let start = remaining.iter().next().unwrap();
    let mut visited: Vec<String> = vec![start.clone()];
    let mut visited_set: HashSet<String> = [start.clone()].into_iter().collect();
    let mut current = start.as_str();

    loop {
        let nexts = dep_map.get(current).cloned().unwrap_or_default();
        if nexts.is_empty() {
            break;
        }
        let nxt = nexts[0];
        if visited_set.contains(nxt) {
            let idx = visited.iter().position(|v| v == nxt).unwrap();
            let mut cycle = visited[idx..].to_vec();
            cycle.push(nxt.to_string());
            return cycle;
        }
        visited.push(nxt.to_string());
        visited_set.insert(nxt.to_string());
        current = nxt;
    }

    // Fallback
    let first = remaining.iter().next().unwrap().clone();
    let mut result: Vec<String> = remaining.iter().cloned().collect();
    result.push(first);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_modules() {
        let result = resolve_dependencies(&[], None).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_no_dependencies() {
        let modules = vec![("a".to_string(), vec![]), ("b".to_string(), vec![])];
        let result = resolve_dependencies(&modules, None).unwrap();
        assert_eq!(result, vec!["a", "b"]);
    }

    #[test]
    fn test_linear_dependencies() {
        let modules = vec![
            (
                "b".to_string(),
                vec![DepInfo {
                    module_id: "a".to_string(),
                    version: None,
                    optional: false,
                }],
            ),
            ("a".to_string(), vec![]),
        ];
        let result = resolve_dependencies(&modules, None).unwrap();
        let a_pos = result.iter().position(|x| x == "a").unwrap();
        let b_pos = result.iter().position(|x| x == "b").unwrap();
        assert!(a_pos < b_pos);
    }

    #[test]
    fn test_circular_dependency() {
        let modules = vec![
            (
                "a".to_string(),
                vec![DepInfo {
                    module_id: "b".to_string(),
                    version: None,
                    optional: false,
                }],
            ),
            (
                "b".to_string(),
                vec![DepInfo {
                    module_id: "a".to_string(),
                    version: None,
                    optional: false,
                }],
            ),
        ];
        let result = resolve_dependencies(&modules, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_missing_required_dependency() {
        let modules = vec![(
            "a".to_string(),
            vec![DepInfo {
                module_id: "missing".to_string(),
                version: None,
                optional: false,
            }],
        )];
        let result = resolve_dependencies(&modules, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_missing_optional_dependency() {
        let modules = vec![(
            "a".to_string(),
            vec![DepInfo {
                module_id: "missing".to_string(),
                version: None,
                optional: true,
            }],
        )];
        let result = resolve_dependencies(&modules, None).unwrap();
        assert_eq!(result, vec!["a"]);
    }
}

// APCore Protocol — Schema reference resolver
// Spec reference: JSON $ref resolution and circular reference detection

use std::collections::{HashMap, HashSet};

use crate::errors::{ErrorCode, ModuleError, SchemaCircularRefError};

/// Default maximum depth for `$ref` resolution. Matches apcore-python
/// (`schema.max_ref_depth = 32`) and apcore-typescript.
pub const DEFAULT_MAX_REF_DEPTH: usize = 32;

/// Resolves $ref references in JSON schemas.
#[derive(Debug)]
pub struct RefResolver {
    schemas: HashMap<String, serde_json::Value>,
    max_depth: usize,
}

impl RefResolver {
    /// Create a new ref resolver with the default max depth.
    #[must_use]
    pub fn new() -> Self {
        Self {
            schemas: HashMap::new(),
            max_depth: DEFAULT_MAX_REF_DEPTH,
        }
    }

    /// Create a ref resolver with an explicit `max_depth` for `$ref` recursion.
    #[must_use]
    pub fn with_max_depth(max_depth: usize) -> Self {
        Self {
            schemas: HashMap::new(),
            max_depth,
        }
    }

    /// Returns the configured maximum recursion depth for `$ref` resolution.
    #[must_use]
    pub fn max_depth(&self) -> usize {
        self.max_depth
    }

    /// Register a schema that can be referenced.
    pub fn register(&mut self, uri: &str, schema: serde_json::Value) {
        self.schemas.insert(uri.to_string(), schema);
    }

    /// Resolve all $ref references in a schema, returning a fully dereferenced schema.
    pub fn resolve(&self, schema: &serde_json::Value) -> Result<serde_json::Value, ModuleError> {
        let mut seen = HashSet::new();
        self.resolve_inner(schema, schema, &mut seen, 0)
    }

    /// Check if a schema contains circular references.
    #[must_use]
    pub fn has_circular_refs(&self, schema: &serde_json::Value) -> bool {
        let mut seen = HashSet::new();
        self.check_circular(schema, schema, &mut seen)
    }

    /// Recursively resolve $ref nodes.
    fn resolve_inner(
        &self,
        node: &serde_json::Value,
        root: &serde_json::Value,
        seen: &mut HashSet<String>,
        depth: usize,
    ) -> Result<serde_json::Value, ModuleError> {
        if depth >= self.max_depth {
            return Err(SchemaCircularRefError::new(
                format!(
                    "Schema $ref recursion exceeded max_depth={} (sync SCHEMA-001)",
                    self.max_depth
                ),
                "<max_depth>".to_string(),
            )
            .to_module_error());
        }
        match node {
            serde_json::Value::Object(map) => {
                // If this node is a $ref, resolve it
                if let Some(ref_val) = map.get("$ref") {
                    if let Some(ref_str) = ref_val.as_str() {
                        if seen.contains(ref_str) {
                            return Err(SchemaCircularRefError::new(
                                format!("Circular $ref detected: {ref_str}"),
                                ref_str.to_string(),
                            )
                            .to_module_error());
                        }
                        seen.insert(ref_str.to_string());

                        let resolved = self.lookup_ref(ref_str, root)?;
                        let result = self.resolve_inner(&resolved, root, seen, depth + 1)?;
                        seen.remove(ref_str);
                        return Ok(result);
                    }
                }

                // Otherwise walk all children
                let mut new_map = serde_json::Map::new();
                for (k, v) in map {
                    new_map.insert(k.clone(), self.resolve_inner(v, root, seen, depth + 1)?);
                }
                Ok(serde_json::Value::Object(new_map))
            }
            serde_json::Value::Array(arr) => {
                let resolved: Result<Vec<_>, _> = arr
                    .iter()
                    .map(|v| self.resolve_inner(v, root, seen, depth + 1))
                    .collect();
                Ok(serde_json::Value::Array(resolved?))
            }
            other => Ok(other.clone()),
        }
    }

    /// Look up a $ref string, supporting local (#/definitions/..., #/$defs/...)
    /// and registered URI references.
    fn lookup_ref(
        &self,
        ref_str: &str,
        root: &serde_json::Value,
    ) -> Result<serde_json::Value, ModuleError> {
        if let Some(pointer) = ref_str.strip_prefix('#') {
            // Local reference: walk the JSON pointer path
            if pointer.is_empty() {
                return Ok(root.clone());
            }
            root.pointer(pointer).cloned().ok_or_else(|| {
                ModuleError::new(
                    ErrorCode::SchemaNotFound,
                    format!("Local $ref not found: {ref_str}"),
                )
            })
        } else {
            // Registered URI reference
            self.schemas.get(ref_str).cloned().ok_or_else(|| {
                ModuleError::new(
                    ErrorCode::SchemaNotFound,
                    format!("Referenced schema not found: {ref_str}"),
                )
            })
        }
    }

    /// Recursively check for circular $ref paths.
    fn check_circular(
        &self,
        node: &serde_json::Value,
        root: &serde_json::Value,
        seen: &mut HashSet<String>,
    ) -> bool {
        match node {
            serde_json::Value::Object(map) => {
                if let Some(ref_val) = map.get("$ref") {
                    if let Some(ref_str) = ref_val.as_str() {
                        if seen.contains(ref_str) {
                            return true;
                        }
                        seen.insert(ref_str.to_string());

                        if let Ok(resolved) = self.lookup_ref(ref_str, root) {
                            let circular = self.check_circular(&resolved, root, seen);
                            seen.remove(ref_str);
                            return circular;
                        }
                        seen.remove(ref_str);
                        return false;
                    }
                }
                for v in map.values() {
                    if self.check_circular(v, root, seen) {
                        return true;
                    }
                }
                false
            }
            serde_json::Value::Array(arr) => {
                for v in arr {
                    if self.check_circular(v, root, seen) {
                        return true;
                    }
                }
                false
            }
            _ => false,
        }
    }
}

impl Default for RefResolver {
    fn default() -> Self {
        Self::new()
    }
}

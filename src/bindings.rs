// APCore Protocol — Binding loader
// Spec reference: Module binding resolution from external sources

use serde::{Deserialize, Serialize};
use serde_yaml_ng as serde_yaml;
use std::collections::HashMap;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use crate::context::Context;
use crate::decorator::FunctionModule;
use crate::errors::ModuleError;
use crate::module::ModuleAnnotations;
use crate::registry::registry::Registry;

/// Boxed async handler type used by [`BindingLoader::register_into_with_handlers`].
///
/// The handler takes the module inputs plus a reference to the execution
/// context and returns a JSON result. Handlers are stored as `Arc` so they
/// can be cheaply cloned when materializing multiple modules.
pub type BindingHandler = Arc<
    dyn for<'a> Fn(
            serde_json::Value,
            &'a Context<serde_json::Value>,
        ) -> Pin<
            Box<
                dyn std::future::Future<Output = Result<serde_json::Value, ModuleError>>
                    + Send
                    + 'a,
            >,
        > + Send
        + Sync,
>;

/// Describes a binding target.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BindingTarget {
    pub module_name: String,
    pub callable: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema_path: Option<String>,
}

/// A resolved binding definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BindingDefinition {
    pub name: String,
    pub target: BindingTarget,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Loads and resolves module bindings from files or configuration.
#[derive(Debug)]
pub struct BindingLoader {
    bindings: HashMap<String, BindingDefinition>,
}

impl BindingLoader {
    /// Create a new empty binding loader.
    pub fn new() -> Self {
        Self {
            bindings: HashMap::new(),
        }
    }

    /// Load bindings from a JSON file.
    pub fn load_from_file(&mut self, path: &Path) -> Result<(), ModuleError> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            ModuleError::new(
                crate::errors::ErrorCode::BindingFileInvalid,
                format!("Failed to read binding file '{}': {}", path.display(), e),
            )
        })?;

        let definitions: Vec<BindingDefinition> = serde_json::from_str(&content).map_err(|e| {
            ModuleError::new(
                crate::errors::ErrorCode::BindingFileInvalid,
                format!("Failed to parse binding file '{}': {}", path.display(), e),
            )
        })?;

        for def in definitions {
            self.bindings.insert(def.name.clone(), def);
        }

        Ok(())
    }

    /// Load bindings from a YAML file.
    pub fn load_from_yaml(&mut self, path: &Path) -> Result<(), ModuleError> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            ModuleError::new(
                crate::errors::ErrorCode::BindingFileInvalid,
                format!(
                    "Failed to read binding YAML file '{}': {}",
                    path.display(),
                    e
                ),
            )
        })?;

        let definitions: Vec<BindingDefinition> = serde_yaml::from_str(&content).map_err(|e| {
            ModuleError::new(
                crate::errors::ErrorCode::BindingFileInvalid,
                format!(
                    "Failed to parse binding YAML file '{}': {}",
                    path.display(),
                    e
                ),
            )
        })?;

        for def in definitions {
            self.bindings.insert(def.name.clone(), def);
        }

        Ok(())
    }

    /// Load all YAML binding files matching a glob pattern from a directory.
    ///
    /// Scans `dir` for files whose names match `pattern` (default
    /// `"*.binding.yaml"`), calls [`load_from_yaml`](Self::load_from_yaml) on
    /// each match, and returns the total number of bindings loaded.
    pub fn load_binding_dir(
        &mut self,
        dir: &Path,
        pattern: Option<&str>,
    ) -> Result<usize, ModuleError> {
        let pattern = pattern.unwrap_or("*.binding.yaml");

        if !dir.is_dir() {
            return Err(ModuleError::new(
                crate::errors::ErrorCode::BindingFileInvalid,
                format!(
                    "Binding directory '{}' does not exist or is not a directory",
                    dir.display()
                ),
            ));
        }

        // Convert the glob pattern to a simple suffix match.
        // Patterns like "*.binding.yaml" become a suffix check on ".binding.yaml".
        let suffix = pattern.strip_prefix('*').unwrap_or(pattern);

        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .map_err(|e| {
                ModuleError::new(
                    crate::errors::ErrorCode::BindingFileInvalid,
                    format!("Failed to read directory '{}': {}", dir.display(), e),
                )
            })?
            .filter_map(std::result::Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.ends_with(suffix))
            })
            .collect();

        // Sort for deterministic load order (matches Python's sorted(p.glob(…))).
        entries.sort_by_key(std::fs::DirEntry::file_name);

        let before = self.bindings.len();
        for entry in entries {
            self.load_from_yaml(&entry.path())?;
        }
        Ok(self.bindings.len() - before)
    }

    /// Resolve a binding by name.
    pub fn resolve(&self, name: &str) -> Result<&BindingDefinition, ModuleError> {
        self.bindings.get(name).ok_or_else(|| {
            ModuleError::new(
                crate::errors::ErrorCode::BindingModuleNotFound,
                format!("Binding '{name}' not found"),
            )
        })
    }

    /// List all loaded binding names.
    pub fn list_bindings(&self) -> Vec<&str> {
        self.bindings
            .keys()
            .map(std::string::String::as_str)
            .collect()
    }

    /// Register every loaded binding as a [`FunctionModule`] in `registry`,
    /// resolving each binding's executable handler from `handlers`.
    ///
    /// Python resolves `BindingTarget::callable` via dynamic import; Rust
    /// cannot do that, so the caller supplies a map from binding name to
    /// handler closure. Each binding is registered under its `name` and the
    /// method returns the number of modules registered.
    ///
    /// Returns an error if any binding is missing a handler or if the
    /// underlying [`Registry::register_module`] call fails.
    #[allow(clippy::needless_pass_by_value)] // public API: HashMap consumed to prevent reuse after registration
    pub fn register_into_with_handlers(
        &self,
        registry: &Registry,
        handlers: HashMap<String, BindingHandler>,
    ) -> Result<usize, ModuleError> {
        let mut count = 0usize;
        for (name, def) in &self.bindings {
            let handler = handlers.get(name).cloned().ok_or_else(|| {
                ModuleError::new(
                    crate::errors::ErrorCode::BindingModuleNotFound,
                    format!("No handler provided for binding '{name}'"),
                )
            })?;

            let description = def
                .metadata
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let module = FunctionModule::with_description(
                ModuleAnnotations::default(),
                serde_json::json!({"type": "object"}),
                serde_json::json!({"type": "object"}),
                description,
                None,
                Vec::new(),
                "0.1.0",
                HashMap::new(),
                Vec::new(),
                move |inputs, ctx| {
                    let handler = Arc::clone(&handler);
                    Box::pin(async move { (handler)(inputs, ctx).await })
                },
            );

            registry.register_module(def.target.module_name.as_str(), Box::new(module))?;
            count += 1;
        }
        Ok(count)
    }
}

impl Default for BindingLoader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_binding_loader_new_is_empty() {
        let loader = BindingLoader::new();
        assert!(loader.list_bindings().is_empty());
    }

    #[test]
    fn test_binding_loader_default() {
        let loader = BindingLoader::default();
        assert!(loader.list_bindings().is_empty());
    }

    #[test]
    fn test_resolve_missing_binding() {
        let loader = BindingLoader::new();
        let result = loader.resolve("nonexistent");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, crate::errors::ErrorCode::BindingModuleNotFound);
        assert!(err.message.contains("nonexistent"));
    }

    #[test]
    fn test_load_from_file_json() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("bindings.json");

        let bindings = json!([
            {
                "name": "send_email",
                "target": {
                    "module_name": "executor.email.send",
                    "callable": "send_handler"
                },
                "metadata": {"description": "Send an email"}
            },
            {
                "name": "fetch_data",
                "target": {
                    "module_name": "executor.data.fetch",
                    "callable": "fetch_handler"
                },
                "metadata": {}
            }
        ]);
        std::fs::write(&file_path, serde_json::to_string(&bindings).unwrap()).unwrap();

        let mut loader = BindingLoader::new();
        loader.load_from_file(&file_path).unwrap();

        assert_eq!(loader.list_bindings().len(), 2);

        let def = loader.resolve("send_email").unwrap();
        assert_eq!(def.target.module_name, "executor.email.send");
        assert_eq!(def.target.callable, "send_handler");
    }

    #[test]
    fn test_load_from_file_invalid_path() {
        let mut loader = BindingLoader::new();
        let result = loader.load_from_file(Path::new("/nonexistent/bindings.json"));
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().code,
            crate::errors::ErrorCode::BindingFileInvalid
        );
    }

    #[test]
    fn test_load_from_file_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("bad.json");
        std::fs::write(&file_path, "not json at all").unwrap();

        let mut loader = BindingLoader::new();
        let result = loader.load_from_file(&file_path);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().code,
            crate::errors::ErrorCode::BindingFileInvalid
        );
    }

    #[test]
    fn test_load_from_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("bindings.yaml");

        let yaml_content = r"
- name: greet
  target:
    module_name: executor.greet
    callable: greet_fn
  metadata: {}
";
        std::fs::write(&file_path, yaml_content).unwrap();

        let mut loader = BindingLoader::new();
        loader.load_from_yaml(&file_path).unwrap();

        assert_eq!(loader.list_bindings().len(), 1);
        let def = loader.resolve("greet").unwrap();
        assert_eq!(def.target.module_name, "executor.greet");
    }

    #[test]
    fn test_load_from_yaml_invalid_path() {
        let mut loader = BindingLoader::new();
        let result = loader.load_from_yaml(Path::new("/no/such/file.yaml"));
        assert!(result.is_err());
    }

    #[test]
    fn test_load_from_yaml_invalid_content() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("bad.yaml");
        std::fs::write(&file_path, "- [invalid structure").unwrap();

        let mut loader = BindingLoader::new();
        let result = loader.load_from_yaml(&file_path);
        assert!(result.is_err());
    }

    #[test]
    fn test_binding_target_serde() {
        let target = BindingTarget {
            module_name: "executor.foo".to_string(),
            callable: "bar".to_string(),
            schema_path: Some("schemas/foo.json".to_string()),
        };
        let json = serde_json::to_value(&target).unwrap();
        assert_eq!(json["module_name"], "executor.foo");
        assert_eq!(json["schema_path"], "schemas/foo.json");

        let deserialized: BindingTarget = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.module_name, "executor.foo");
        assert_eq!(
            deserialized.schema_path.as_deref(),
            Some("schemas/foo.json")
        );
    }

    #[test]
    fn test_binding_target_schema_path_none_omitted() {
        let target = BindingTarget {
            module_name: "m".to_string(),
            callable: "c".to_string(),
            schema_path: None,
        };
        let json = serde_json::to_value(&target).unwrap();
        assert!(json.get("schema_path").is_none());
    }

    #[test]
    fn test_load_binding_dir_default_pattern() {
        let dir = tempfile::tempdir().unwrap();

        // Matching files
        let yaml_a = r"
- name: alpha
  target:
    module_name: executor.alpha
    callable: alpha_fn
  metadata: {}
";
        let yaml_b = r"
- name: beta
  target:
    module_name: executor.beta
    callable: beta_fn
  metadata: {}
";
        std::fs::write(dir.path().join("a.binding.yaml"), yaml_a).unwrap();
        std::fs::write(dir.path().join("b.binding.yaml"), yaml_b).unwrap();

        // Non-matching file — should be ignored
        std::fs::write(dir.path().join("ignored.yaml"), yaml_a).unwrap();

        let mut loader = BindingLoader::new();
        let count = loader.load_binding_dir(dir.path(), None).unwrap();
        assert_eq!(count, 2);
        assert!(loader.resolve("alpha").is_ok());
        assert!(loader.resolve("beta").is_ok());
    }

    #[test]
    fn test_load_binding_dir_custom_pattern() {
        let dir = tempfile::tempdir().unwrap();

        let yaml = r"
- name: gamma
  target:
    module_name: executor.gamma
    callable: gamma_fn
  metadata: {}
";
        std::fs::write(dir.path().join("gamma.custom.yml"), yaml).unwrap();
        std::fs::write(dir.path().join("gamma.binding.yaml"), yaml).unwrap();

        let mut loader = BindingLoader::new();
        let count = loader
            .load_binding_dir(dir.path(), Some("*.custom.yml"))
            .unwrap();
        assert_eq!(count, 1);
        assert!(loader.resolve("gamma").is_ok());
    }

    #[test]
    fn test_load_binding_dir_nonexistent() {
        let mut loader = BindingLoader::new();
        let result = loader.load_binding_dir(Path::new("/no/such/dir"), None);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().code,
            crate::errors::ErrorCode::BindingFileInvalid
        );
    }

    #[test]
    fn test_load_binding_dir_empty_directory() {
        let dir = tempfile::tempdir().unwrap();
        let mut loader = BindingLoader::new();
        let count = loader.load_binding_dir(dir.path(), None).unwrap();
        assert_eq!(count, 0);
        assert!(loader.list_bindings().is_empty());
    }

    #[test]
    fn test_load_overwrites_duplicate_names() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("bindings.json");

        let bindings = json!([
            {
                "name": "dup",
                "target": {"module_name": "first", "callable": "a"},
                "metadata": {}
            },
            {
                "name": "dup",
                "target": {"module_name": "second", "callable": "b"},
                "metadata": {}
            }
        ]);
        std::fs::write(&file_path, serde_json::to_string(&bindings).unwrap()).unwrap();

        let mut loader = BindingLoader::new();
        loader.load_from_file(&file_path).unwrap();

        let def = loader.resolve("dup").unwrap();
        assert_eq!(def.target.module_name, "second");
    }
}

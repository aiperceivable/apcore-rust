// APCore Protocol — Binding loader
// Spec reference: DECLARATIVE_CONFIG_SPEC.md §3 (Bindings YAML)
//
// Cross-language note: Rust cannot dynamically import compiled modules at
// runtime, so the canonical `target: "module:callable"` string is used as
// an opaque key into a user-supplied handler map. The YAML syntax itself
// is byte-identical across Python, TypeScript, and Rust SDKs.

use serde::{Deserialize, Serialize};
use serde_yaml_ng as serde_yaml;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use crate::context::Context;
use crate::decorator::FunctionModule;
use crate::errors::{ErrorCode, ModuleError};
use crate::module::ModuleAnnotations;
use crate::registry::registry::Registry;

const CURRENT_SPEC_VERSION: &str = "1.0";

const SUPPORTED_SPEC_VERSIONS: &[&str] = &["1.0"];

/// Boxed async handler function type.
///
/// The handler takes the module inputs plus a reference to the execution
/// context and returns a JSON result. Handlers are stored as `Arc` so they
/// can be cheaply cloned when materializing multiple modules.
pub type BindingHandlerFn = Arc<
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

/// Backward-compatible alias.
pub type BindingHandler = BindingHandlerFn;

/// A binding handler bundled with optional auto-derived schemas.
///
/// When `auto_schema: true` is specified in a binding entry, the loader
/// reads schemas from this struct instead of falling back to a permissive
/// `{"type":"object"}`. Use [`typed_handler`] to create instances with
/// auto-generated schemas from `schemars::JsonSchema` types.
pub struct TypedBindingHandler {
    pub handler: BindingHandlerFn,
    pub input_schema: Option<serde_json::Value>,
    pub output_schema: Option<serde_json::Value>,
}

/// Create a [`TypedBindingHandler`] with auto-derived JSON Schemas.
///
/// The input and output types must implement both `schemars::JsonSchema`
/// (for auto-schema derivation) and the standard serde traits (for
/// runtime de/serialization).
///
/// # Example
///
/// ```ignore
/// use apcore::bindings::typed_handler;
/// use schemars::JsonSchema;
///
/// #[derive(serde::Deserialize, JsonSchema)]
/// struct Input { name: String }
///
/// #[derive(serde::Serialize, JsonSchema)]
/// struct Output { greeting: String }
///
/// let handler = typed_handler::<Input, Output>(|input| {
///     Ok(Output { greeting: format!("Hello, {}!", input.name) })
/// });
/// ```
pub fn typed_handler<I, O>(
    f: impl Fn(I) -> Result<O, ModuleError> + Send + Sync + 'static,
) -> TypedBindingHandler
where
    I: schemars::JsonSchema + serde::de::DeserializeOwned + Send + 'static,
    O: schemars::JsonSchema + serde::Serialize + Send + 'static,
{
    let f = Arc::new(f);
    let handler: BindingHandlerFn = Arc::new(move |input: serde_json::Value, _ctx| {
        let f = Arc::clone(&f);
        Box::pin(async move {
            let typed: I = serde_json::from_value(input).map_err(|e| {
                ModuleError::new(
                    ErrorCode::GeneralInvalidInput,
                    format!("Failed to deserialize input: {e}"),
                )
            })?;
            let result = f(typed)?;
            serde_json::to_value(result).map_err(|e| {
                ModuleError::new(
                    ErrorCode::GeneralInternalError,
                    format!("Failed to serialize output: {e}"),
                )
            })
        })
    });

    let input_schema = Some(serde_json::to_value(schemars::schema_for!(I)).unwrap_or_default());
    let output_schema = Some(serde_json::to_value(schemars::schema_for!(O)).unwrap_or_default());

    TypedBindingHandler {
        handler,
        input_schema,
        output_schema,
    }
}

/// `auto_schema` field accepts either a boolean or a mode string.
///
/// `true` is a synonym for `"permissive"`. See `DECLARATIVE_CONFIG_SPEC.md` §6.2.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum AutoSchemaValue {
    Bool(bool),
    Mode(String),
}

impl AutoSchemaValue {
    /// Normalize to canonical mode string: `"permissive"` or `"strict"`.
    /// Returns `None` when explicitly disabled (`false`).
    pub fn normalize(&self) -> Result<Option<&str>, String> {
        match self {
            Self::Bool(true) => Ok(Some("permissive")),
            Self::Bool(false) => Ok(None),
            Self::Mode(s) => match s.as_str() {
                "true" | "permissive" => Ok(Some("permissive")),
                "strict" => Ok(Some("strict")),
                other => Err(format!(
                    "auto_schema must be a boolean or one of [\"true\", \"permissive\", \"strict\"]; got {other:?}"
                )),
            },
        }
    }
}

/// A single binding entry. Mirrors the canonical YAML structure defined in
/// `DECLARATIVE_CONFIG_SPEC.md` §3.3.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BindingEntry {
    pub module_id: String,
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub documentation: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_schema: Option<AutoSchemaValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, serde_json::Value>,
}

fn default_version() -> String {
    "1.0.0".to_string()
}

/// Top-level binding file structure: `spec_version` + `bindings:` list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BindingsFile {
    #[serde(default)]
    pub spec_version: Option<String>,
    pub bindings: Vec<BindingEntry>,
}

/// In-memory resolved schema pair for a binding.
#[derive(Debug, Clone)]
struct ResolvedSchemas {
    input: serde_json::Value,
    output: serde_json::Value,
}

/// Loads and resolves module bindings from `*.binding.yaml` files.
#[derive(Debug)]
pub struct BindingLoader {
    /// Registered binding entries keyed by `module_id`.
    bindings: HashMap<String, BindingEntry>,
    /// Resolved schemas (after `schema_ref` loading) keyed by `module_id`.
    schemas: HashMap<String, ResolvedSchemas>,
}

impl BindingLoader {
    /// Create a new empty binding loader.
    #[must_use]
    pub fn new() -> Self {
        Self {
            bindings: HashMap::new(),
            schemas: HashMap::new(),
        }
    }

    /// Load bindings from a JSON file. Same canonical structure as YAML.
    pub fn load_from_file(&mut self, path: &Path) -> Result<(), ModuleError> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            ModuleError::new(
                ErrorCode::BindingFileInvalid,
                format!("Failed to read binding file '{}': {}", path.display(), e),
            )
        })?;
        let file: BindingsFile = serde_json::from_str(&content).map_err(|e| {
            ModuleError::new(
                ErrorCode::BindingFileInvalid,
                format!("Failed to parse binding JSON '{}': {}", path.display(), e),
            )
        })?;
        self.ingest(file, path)
    }

    /// Load bindings from a YAML file.
    pub fn load_from_yaml(&mut self, path: &Path) -> Result<(), ModuleError> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            ModuleError::new(
                ErrorCode::BindingFileInvalid,
                format!("Failed to read binding YAML '{}': {}", path.display(), e),
            )
        })?;
        let file: BindingsFile = serde_yaml::from_str(&content).map_err(|e| {
            ModuleError::new(
                ErrorCode::BindingFileInvalid,
                format!("Failed to parse binding YAML '{}': {}", path.display(), e),
            )
        })?;
        self.ingest(file, path)
    }

    /// Common ingestion path after parsing into [`BindingsFile`].
    fn ingest(&mut self, file: BindingsFile, source_path: &Path) -> Result<(), ModuleError> {
        match file.spec_version.as_deref() {
            None => {
                tracing::warn!(
                    path = %source_path.display(),
                    default_version = CURRENT_SPEC_VERSION,
                    "spec_version missing in bindings file; defaulting. \
                     spec_version will be mandatory in spec 1.1. \
                     See DECLARATIVE_CONFIG_SPEC.md §2.4"
                );
            }
            Some(v) if !SUPPORTED_SPEC_VERSIONS.contains(&v) => {
                tracing::warn!(
                    path = %source_path.display(),
                    spec_version = v,
                    supported = ?SUPPORTED_SPEC_VERSIONS,
                    "bindings spec_version is newer than supported; proceeding best-effort"
                );
            }
            _ => {}
        }

        let dir = source_path.parent().unwrap_or_else(|| Path::new("."));
        for entry in file.bindings {
            let module_id = entry.module_id.clone();
            let schemas = self.resolve_schemas(&entry, dir, source_path)?;
            self.schemas.insert(module_id.clone(), schemas);
            self.bindings.insert(module_id, entry);
        }
        Ok(())
    }

    /// Resolve input/output schemas per `DECLARATIVE_CONFIG_SPEC.md` §3.4.
    ///
    /// Detects mode conflicts (multiple schema fields specified together)
    /// and loads `schema_ref` external files. For Rust, `auto_schema` is
    /// recorded but produces an empty/permissive schema until apcore-macros
    /// (F11) wires up `schemars`-derived lookup.
    #[allow(clippy::unused_self)]
    fn resolve_schemas(
        &self,
        entry: &BindingEntry,
        binding_dir: &Path,
        source_path: &Path,
    ) -> Result<ResolvedSchemas, ModuleError> {
        let modes = detect_modes(entry);
        if modes.len() > 1 {
            return Err(ModuleError::new(
                ErrorCode::BindingSchemaModeConflict,
                format!(
                    "{}: binding '{}' specifies multiple schema modes ({}). Choose one. See DECLARATIVE_CONFIG_SPEC.md §3.4",
                    source_path.display(),
                    entry.module_id,
                    modes.join(", "),
                ),
            ));
        }

        // Mode 1: explicit input/output_schema
        if entry.input_schema.is_some() || entry.output_schema.is_some() {
            let input = entry.input_schema.clone();
            let output = entry.output_schema.clone();
            if input.is_none() || output.is_none() {
                return Err(ModuleError::new(
                    ErrorCode::BindingFileInvalid,
                    format!(
                        "{}: binding '{}': explicit schema mode requires both 'input_schema' and 'output_schema'",
                        source_path.display(),
                        entry.module_id,
                    ),
                ));
            }
            return Ok(ResolvedSchemas {
                input: input.unwrap(),
                output: output.unwrap(),
            });
        }

        // Mode 2: external file reference
        if let Some(ref_str) = &entry.schema_ref {
            let ref_path: PathBuf = binding_dir.join(ref_str);
            let ref_content = std::fs::read_to_string(&ref_path).map_err(|e| {
                ModuleError::new(
                    ErrorCode::BindingFileInvalid,
                    format!(
                        "{}: schema_ref file '{}' not readable: {}",
                        source_path.display(),
                        ref_path.display(),
                        e,
                    ),
                )
            })?;
            let ref_doc: serde_yaml::Value = serde_yaml::from_str(&ref_content).map_err(|e| {
                ModuleError::new(
                    ErrorCode::BindingFileInvalid,
                    format!(
                        "{}: schema_ref file '{}' YAML parse error: {}",
                        source_path.display(),
                        ref_path.display(),
                        e,
                    ),
                )
            })?;
            let input = serde_yaml::from_value::<serde_json::Value>(
                ref_doc
                    .get("input_schema")
                    .cloned()
                    .unwrap_or(serde_yaml::Value::Null),
            )
            .unwrap_or(serde_json::Value::Object(serde_json::Map::default()));
            let output = serde_yaml::from_value::<serde_json::Value>(
                ref_doc
                    .get("output_schema")
                    .cloned()
                    .unwrap_or(serde_yaml::Value::Null),
            )
            .unwrap_or(serde_json::Value::Object(serde_json::Map::default()));
            return Ok(ResolvedSchemas { input, output });
        }

        // Mode 3: explicit auto_schema (any value) OR implicit default
        let auto_mode = match &entry.auto_schema {
            Some(v) => v.normalize().map_err(|reason| {
                ModuleError::new(
                    ErrorCode::BindingFileInvalid,
                    format!(
                        "{}: binding '{}': {}",
                        source_path.display(),
                        entry.module_id,
                        reason
                    ),
                )
            })?,
            None => None,
        };

        // auto_schema explicitly false → no mode left, error
        if entry.auto_schema.is_some() && auto_mode.is_none() {
            return Err(ModuleError::new(
                ErrorCode::BindingSchemaInferenceFailed,
                format!(
                    "{}: binding '{}': auto_schema is explicitly false; provide input_schema/output_schema or schema_ref instead. See DECLARATIVE_CONFIG_SPEC.md §3.4",
                    source_path.display(),
                    entry.module_id,
                ),
            ));
        }

        // Implicit default: auto_schema permissive.
        // Rust auto-inference requires apcore-macros + schemars (F11). Until
        // wired, we yield a permissive object schema and rely on the user's
        // handler to validate inputs.
        let _resolved_mode = auto_mode.unwrap_or("permissive");
        Ok(ResolvedSchemas {
            input: serde_json::json!({"type": "object"}),
            output: serde_json::json!({"type": "object"}),
        })
    }

    /// Load all YAML binding files matching `pattern` in `dir`.
    pub fn load_binding_dir(
        &mut self,
        dir: &Path,
        pattern: Option<&str>,
    ) -> Result<usize, ModuleError> {
        let pattern = pattern.unwrap_or("*.binding.yaml");

        if !dir.is_dir() {
            return Err(ModuleError::new(
                ErrorCode::BindingFileInvalid,
                format!(
                    "Binding directory '{}' does not exist or is not a directory",
                    dir.display()
                ),
            ));
        }

        let suffix = pattern.strip_prefix('*').unwrap_or(pattern);

        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .map_err(|e| {
                ModuleError::new(
                    ErrorCode::BindingFileInvalid,
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

        entries.sort_by_key(std::fs::DirEntry::file_name);

        let before = self.bindings.len();
        for entry in entries {
            self.load_from_yaml(&entry.path())?;
        }
        Ok(self.bindings.len() - before)
    }

    /// Resolve a binding by `module_id`.
    pub fn resolve(&self, module_id: &str) -> Result<&BindingEntry, ModuleError> {
        self.bindings.get(module_id).ok_or_else(|| {
            ModuleError::new(
                ErrorCode::BindingModuleNotFound,
                format!("Binding '{module_id}' not found"),
            )
        })
    }

    /// List all loaded binding `module_id`s.
    pub fn list_bindings(&self) -> Vec<&str> {
        self.bindings
            .keys()
            .map(std::string::String::as_str)
            .collect()
    }

    /// Register every loaded binding as a [`FunctionModule`] in `registry`,
    /// using `handlers` keyed by the binding's full `target` string.
    ///
    /// Per `DECLARATIVE_CONFIG_SPEC.md` §3.7, Rust treats the `target` string
    /// as an opaque handler-map key. The user is responsible for providing a
    /// closure for every `target` referenced by the loaded YAML.
    ///
    /// Returns the number of modules registered, or an error if any binding
    /// is missing a handler.
    #[allow(clippy::needless_pass_by_value)]
    pub fn register_into_with_handlers(
        &self,
        registry: &Registry,
        handlers: HashMap<String, BindingHandler>,
    ) -> Result<usize, ModuleError> {
        let mut count = 0usize;
        for (module_id, entry) in &self.bindings {
            let handler = handlers.get(&entry.target).cloned().ok_or_else(|| {
                ModuleError::new(
                    ErrorCode::BindingModuleNotFound,
                    format!(
                        "No handler provided for binding '{}' (target '{}')",
                        module_id, entry.target
                    ),
                )
            })?;

            let schemas = self
                .schemas
                .get(module_id)
                .cloned()
                .unwrap_or(ResolvedSchemas {
                    input: serde_json::json!({"type": "object"}),
                    output: serde_json::json!({"type": "object"}),
                });

            let annotations = annotations_from_value(entry.annotations.as_ref());

            let display_meta = display_into_metadata(entry.display.as_ref());
            let mut metadata = entry.metadata.clone();
            for (k, v) in display_meta {
                metadata.entry(k).or_insert(v);
            }

            let description = entry.description.clone().unwrap_or_default();
            let documentation = entry.documentation.clone();

            let module = FunctionModule::with_description(
                annotations,
                schemas.input,
                schemas.output,
                description,
                documentation,
                entry.tags.clone(),
                entry.version.as_str(),
                metadata,
                Vec::new(),
                move |inputs, ctx| {
                    let handler = Arc::clone(&handler);
                    Box::pin(async move { (handler)(inputs, ctx).await })
                },
            );

            registry.register_module(module_id.as_str(), Box::new(module))?;
            count += 1;
        }
        Ok(count)
    }

    /// Register bindings using [`TypedBindingHandler`]s that carry auto-derived schemas.
    ///
    /// When a binding entry uses `auto_schema` (explicit or implicit default) AND the
    /// corresponding handler carries schemas (`TypedBindingHandler::input_schema` /
    /// `output_schema` are `Some`), the handler's schemas are used instead of the
    /// permissive `{"type":"object"}` fallback. This is the primary mechanism for
    /// Rust `auto_schema` support per `DECLARATIVE_CONFIG_SPEC.md` §6.5.
    ///
    /// For bindings with explicit `input_schema`/`output_schema` or `schema_ref`,
    /// the YAML-specified schemas take precedence (handler schemas are ignored).
    #[allow(clippy::needless_pass_by_value)]
    pub fn register_into_with_typed_handlers(
        &self,
        registry: &Registry,
        handlers: HashMap<String, TypedBindingHandler>,
    ) -> Result<usize, ModuleError> {
        let mut count = 0usize;
        for (module_id, entry) in &self.bindings {
            let typed = handlers.get(&entry.target).ok_or_else(|| {
                ModuleError::new(
                    ErrorCode::BindingModuleNotFound,
                    format!(
                        "No handler provided for binding '{}' (target '{}')",
                        module_id, entry.target
                    ),
                )
            })?;

            // Determine final schemas: YAML-resolved vs handler-provided.
            let yaml_schemas = self.schemas.get(module_id);
            let has_explicit_yaml = entry.input_schema.is_some() || entry.schema_ref.is_some();

            let (input_schema, output_schema) = if has_explicit_yaml {
                // YAML-specified schemas take precedence.
                let s = yaml_schemas.cloned().unwrap_or(ResolvedSchemas {
                    input: serde_json::json!({"type": "object"}),
                    output: serde_json::json!({"type": "object"}),
                });
                (s.input, s.output)
            } else if let (Some(is), Some(os)) = (&typed.input_schema, &typed.output_schema) {
                // Handler provides auto-derived schemas (schemars).
                (is.clone(), os.clone())
            } else {
                // Fallback: permissive.
                (
                    serde_json::json!({"type": "object"}),
                    serde_json::json!({"type": "object"}),
                )
            };

            let annotations = annotations_from_value(entry.annotations.as_ref());
            let display_meta = display_into_metadata(entry.display.as_ref());
            let mut metadata = entry.metadata.clone();
            for (k, v) in display_meta {
                metadata.entry(k).or_insert(v);
            }

            let description = entry.description.clone().unwrap_or_default();
            let documentation = entry.documentation.clone();
            let handler = Arc::clone(&typed.handler);

            let module = FunctionModule::with_description(
                annotations,
                input_schema,
                output_schema,
                description,
                documentation,
                entry.tags.clone(),
                entry.version.as_str(),
                metadata,
                Vec::new(),
                move |inputs, ctx| {
                    let handler = Arc::clone(&handler);
                    Box::pin(async move { (handler)(inputs, ctx).await })
                },
            );

            registry.register_module(module_id.as_str(), Box::new(module))?;
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

/// Detect which schema-mode fields a binding entry sets.
fn detect_modes(entry: &BindingEntry) -> Vec<String> {
    let mut modes = Vec::new();
    if entry.auto_schema.is_some() {
        modes.push("auto_schema".to_string());
    }
    if entry.input_schema.is_some() || entry.output_schema.is_some() {
        modes.push("input_schema/output_schema".to_string());
    }
    if entry.schema_ref.is_some() {
        modes.push("schema_ref".to_string());
    }
    modes
}

/// Translate an `annotations` JSON object into [`ModuleAnnotations`].
///
/// Unknown keys collect into `extra`. Missing or non-object input yields
/// `ModuleAnnotations::default()`.
fn annotations_from_value(value: Option<&serde_json::Value>) -> ModuleAnnotations {
    let mut annotations = ModuleAnnotations::default();
    let Some(serde_json::Value::Object(obj)) = value else {
        return annotations;
    };
    let mut extra = HashMap::new();
    for (k, v) in obj {
        match k.as_str() {
            "readonly" => {
                if let Some(b) = v.as_bool() {
                    annotations.readonly = b;
                }
            }
            "destructive" => {
                if let Some(b) = v.as_bool() {
                    annotations.destructive = b;
                }
            }
            "idempotent" => {
                if let Some(b) = v.as_bool() {
                    annotations.idempotent = b;
                }
            }
            "requires_approval" => {
                if let Some(b) = v.as_bool() {
                    annotations.requires_approval = b;
                }
            }
            "open_world" => {
                if let Some(b) = v.as_bool() {
                    annotations.open_world = b;
                }
            }
            _ => {
                extra.insert(k.clone(), v.clone());
            }
        }
    }
    if !extra.is_empty() {
        annotations.extra = extra;
    }
    annotations
}

/// Move a `display` JSON value into the module's `metadata` namespace under
/// the canonical key `apcore.display`. Surface adapters (CLI, MCP, A2A) read
/// this when rendering the module on a given surface.
fn display_into_metadata(
    display: Option<&serde_json::Value>,
) -> HashMap<String, serde_json::Value> {
    let mut out = HashMap::new();
    if let Some(value) = display {
        out.insert("apcore.display".to_string(), value.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write_yaml(dir: &tempfile::TempDir, name: &str, body: &str) -> PathBuf {
        let p = dir.path().join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

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
        assert_eq!(err.code, ErrorCode::BindingModuleNotFound);
        assert!(err.message.contains("nonexistent"));
    }

    #[test]
    fn test_load_from_yaml_canonical_format() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = r#"
spec_version: "1.0"
bindings:
  - module_id: utils.greet
    target: "greet:greet_fn"
    description: "Greet someone"
    tags: ["util"]
    auto_schema: true
"#;
        let p = write_yaml(&dir, "greet.binding.yaml", yaml);

        let mut loader = BindingLoader::new();
        loader.load_from_yaml(&p).unwrap();
        let entry = loader.resolve("utils.greet").unwrap();
        assert_eq!(entry.target, "greet:greet_fn");
        assert_eq!(entry.description.as_deref(), Some("Greet someone"));
        assert_eq!(entry.tags, vec!["util"]);
    }

    #[test]
    fn test_load_from_json_canonical_format() {
        let dir = tempfile::tempdir().unwrap();
        let body = json!({
            "spec_version": "1.0",
            "bindings": [
                {"module_id": "a.b", "target": "mod:fn", "input_schema": {"type": "object"}, "output_schema": {"type": "object"}}
            ]
        });
        let p = dir.path().join("b.json");
        std::fs::write(&p, serde_json::to_string(&body).unwrap()).unwrap();

        let mut loader = BindingLoader::new();
        loader.load_from_file(&p).unwrap();
        assert_eq!(loader.list_bindings().len(), 1);
    }

    #[test]
    fn test_mode_conflict_auto_schema_plus_input_schema() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = r#"
spec_version: "1.0"
bindings:
  - module_id: x
    target: "m:f"
    auto_schema: true
    input_schema: {type: object}
    output_schema: {type: object}
"#;
        let p = write_yaml(&dir, "x.binding.yaml", yaml);
        let mut loader = BindingLoader::new();
        let err = loader.load_from_yaml(&p).unwrap_err();
        assert_eq!(err.code, ErrorCode::BindingSchemaModeConflict);
        assert!(err.message.contains("multiple schema modes"));
    }

    #[test]
    fn test_mode_conflict_schema_ref_plus_auto_schema() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = r#"
spec_version: "1.0"
bindings:
  - module_id: x
    target: "m:f"
    auto_schema: strict
    schema_ref: "./schema.yaml"
"#;
        let p = write_yaml(&dir, "x.binding.yaml", yaml);
        let mut loader = BindingLoader::new();
        let err = loader.load_from_yaml(&p).unwrap_err();
        assert_eq!(err.code, ErrorCode::BindingSchemaModeConflict);
    }

    #[test]
    fn test_explicit_input_only_is_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = r#"
spec_version: "1.0"
bindings:
  - module_id: x
    target: "m:f"
    input_schema: {type: object}
"#;
        let p = write_yaml(&dir, "x.binding.yaml", yaml);
        let mut loader = BindingLoader::new();
        let err = loader.load_from_yaml(&p).unwrap_err();
        assert_eq!(err.code, ErrorCode::BindingFileInvalid);
        assert!(err.message.contains("requires both"));
    }

    #[test]
    fn test_implicit_auto_schema_default() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = r#"
spec_version: "1.0"
bindings:
  - module_id: x
    target: "m:f"
"#;
        let p = write_yaml(&dir, "x.binding.yaml", yaml);
        let mut loader = BindingLoader::new();
        loader.load_from_yaml(&p).unwrap();
        // Implicit auto: should resolve without error.
        assert_eq!(loader.list_bindings().len(), 1);
    }

    #[test]
    fn test_auto_schema_false_explicit_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = r#"
spec_version: "1.0"
bindings:
  - module_id: x
    target: "m:f"
    auto_schema: false
"#;
        let p = write_yaml(&dir, "x.binding.yaml", yaml);
        let mut loader = BindingLoader::new();
        let err = loader.load_from_yaml(&p).unwrap_err();
        assert_eq!(err.code, ErrorCode::BindingSchemaInferenceFailed);
    }

    #[test]
    fn test_auto_schema_strict_value() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = r#"
spec_version: "1.0"
bindings:
  - module_id: x
    target: "m:f"
    auto_schema: strict
"#;
        let p = write_yaml(&dir, "x.binding.yaml", yaml);
        let mut loader = BindingLoader::new();
        loader.load_from_yaml(&p).unwrap();
        let entry = loader.resolve("x").unwrap();
        match &entry.auto_schema {
            Some(AutoSchemaValue::Mode(m)) => assert_eq!(m, "strict"),
            other => panic!("expected Mode(strict), got {other:?}"),
        }
    }

    #[test]
    fn test_auto_schema_invalid_string_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = r#"
spec_version: "1.0"
bindings:
  - module_id: x
    target: "m:f"
    auto_schema: "not-a-mode"
"#;
        let p = write_yaml(&dir, "x.binding.yaml", yaml);
        let mut loader = BindingLoader::new();
        let err = loader.load_from_yaml(&p).unwrap_err();
        assert_eq!(err.code, ErrorCode::BindingFileInvalid);
        assert!(err.message.contains("not-a-mode"));
    }

    #[test]
    fn test_schema_ref_loads_external_yaml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("schema.yaml"),
            r"
input_schema:
  type: object
  properties:
    name: {type: string}
output_schema:
  type: object
  properties:
    greeting: {type: string}
",
        )
        .unwrap();
        let yaml = r#"
spec_version: "1.0"
bindings:
  - module_id: x
    target: "m:f"
    schema_ref: "./schema.yaml"
"#;
        let p = write_yaml(&dir, "x.binding.yaml", yaml);
        let mut loader = BindingLoader::new();
        loader.load_from_yaml(&p).unwrap();
        let schemas = loader.schemas.get("x").unwrap();
        assert_eq!(schemas.input["properties"]["name"]["type"], "string");
        assert_eq!(schemas.output["properties"]["greeting"]["type"], "string");
    }

    #[test]
    fn test_schema_ref_missing_file_errors() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = r#"
spec_version: "1.0"
bindings:
  - module_id: x
    target: "m:f"
    schema_ref: "./does-not-exist.yaml"
"#;
        let p = write_yaml(&dir, "x.binding.yaml", yaml);
        let mut loader = BindingLoader::new();
        let err = loader.load_from_yaml(&p).unwrap_err();
        assert_eq!(err.code, ErrorCode::BindingFileInvalid);
        assert!(err.message.contains("schema_ref"));
    }

    #[test]
    fn test_load_binding_dir_default_pattern() {
        let dir = tempfile::tempdir().unwrap();
        write_yaml(
            &dir,
            "a.binding.yaml",
            r#"
spec_version: "1.0"
bindings:
  - module_id: alpha
    target: "m.alpha:fn"
"#,
        );
        write_yaml(
            &dir,
            "b.binding.yaml",
            r#"
spec_version: "1.0"
bindings:
  - module_id: beta
    target: "m.beta:fn"
"#,
        );
        write_yaml(
            &dir,
            "ignored.yaml",
            r#"
spec_version: "1.0"
bindings:
  - module_id: ignored
    target: "m.ignored:fn"
"#,
        );

        let mut loader = BindingLoader::new();
        let count = loader.load_binding_dir(dir.path(), None).unwrap();
        assert_eq!(count, 2);
        assert!(loader.resolve("alpha").is_ok());
        assert!(loader.resolve("beta").is_ok());
        assert!(loader.resolve("ignored").is_err());
    }

    #[test]
    fn test_auto_schema_value_normalize() {
        assert_eq!(
            AutoSchemaValue::Bool(true).normalize().unwrap(),
            Some("permissive")
        );
        assert_eq!(AutoSchemaValue::Bool(false).normalize().unwrap(), None);
        assert_eq!(
            AutoSchemaValue::Mode("true".to_string())
                .normalize()
                .unwrap(),
            Some("permissive")
        );
        assert_eq!(
            AutoSchemaValue::Mode("permissive".to_string())
                .normalize()
                .unwrap(),
            Some("permissive")
        );
        assert_eq!(
            AutoSchemaValue::Mode("strict".to_string())
                .normalize()
                .unwrap(),
            Some("strict")
        );
        assert!(AutoSchemaValue::Mode("invalid".to_string())
            .normalize()
            .is_err());
    }

    #[test]
    fn test_annotations_round_trip_through_loader() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = r#"
spec_version: "1.0"
bindings:
  - module_id: x
    target: "m:f"
    annotations:
      readonly: true
      idempotent: true
      destructive: false
"#;
        let p = write_yaml(&dir, "x.binding.yaml", yaml);
        let mut loader = BindingLoader::new();
        loader.load_from_yaml(&p).unwrap();
        let entry = loader.resolve("x").unwrap();
        let ann = entry.annotations.as_ref().unwrap();
        assert_eq!(ann["readonly"], true);
        assert_eq!(ann["idempotent"], true);
        assert_eq!(ann["destructive"], false);
    }

    #[test]
    fn test_display_round_trip_through_loader() {
        let dir = tempfile::tempdir().unwrap();
        let yaml = r#"
spec_version: "1.0"
bindings:
  - module_id: x
    target: "m:f"
    display:
      alias: "x_short"
      cli:
        alias: "x"
"#;
        let p = write_yaml(&dir, "x.binding.yaml", yaml);
        let mut loader = BindingLoader::new();
        loader.load_from_yaml(&p).unwrap();
        let entry = loader.resolve("x").unwrap();
        let display = entry.display.as_ref().unwrap();
        assert_eq!(display["alias"], "x_short");
        assert_eq!(display["cli"]["alias"], "x");
    }
}

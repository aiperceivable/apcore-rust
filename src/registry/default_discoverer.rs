// APCore Protocol — Default discovery pipeline
//
// Wires the building blocks (scanner, metadata, dependencies, validation,
// conflicts) into a Discoverer impl that runs the canonical 8-stage pipeline
// described in `registry-system.md §Discovery`. Cross-language parity with
// apcore-python's `Registry._discover_default` and apcore-typescript's
// `Registry._discoverDefault` (sync finding A-D-005).
//
// Rust limitation: unlike Python/TS which can `import` a discovered file at
// runtime, Rust modules are compiled at build time. The `DefaultDiscoverer`
// therefore takes a user-provided `module_factory` callback that maps
// `(canonical_id, entry_point_name)` to an `Arc<dyn Module>` — typically a
// closure that dispatches over a HashMap of pre-instantiated modules. This
// preserves the spec's pipeline structure (and the spec-mandated error
// surfaces: ConfigNotFoundError, CircularDependencyError) while staying
// honest about what Rust can do.
//
// The pipeline stages (matching the spec):
//   1. scan_extensions          (filesystem scan of every root)
//   2. apply_id_map_overrides   (rewrite canonical_ids per id_map.yaml)
//   3. load_all_metadata        (per-file `_meta.yaml`)
//   4. resolve_entry_points     (struct name from metadata or auto-inferred)
//   5. validate_descriptors     (descriptor structure)
//   6. resolve_load_order       (Kahn's topo sort, dependency version checks)
//   7. detect_id_conflicts      (case-insensitive duplicates, reserved words)
//   8. instantiate via module_factory + build DiscoveredModule list
//
// Stage 6 is where `CircularDependencyError` surfaces. Stage 1 is where
// `ConfigNotFoundError` surfaces (root path missing).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;

use crate::errors::{ErrorCode, ModuleError};
use crate::module::Module;
use crate::registry::dependencies::resolve_dependencies;
use crate::registry::metadata::{load_id_map, load_metadata, parse_dependencies};
use crate::registry::registry::{
    DiscoveredModule, Discoverer, ModuleDescriptor, MAX_MODULE_ID_LENGTH, RESERVED_WORDS,
};
use crate::registry::scanner::scan_extensions;
use crate::registry::types::{DepInfo, DiscoveredFile};

/// Closure type used by `DefaultDiscoverer` to instantiate a module from its
/// resolved entry-point name. Returning `Ok(None)` causes the file to be
/// skipped silently with a `tracing::warn` (the file was discovered but the
/// host application has no factory registered for it). Returning `Err(...)`
/// aborts the discovery pipeline.
pub type ModuleFactory = Arc<
    dyn Fn(&DiscoveredFile, &str) -> Result<Option<Arc<dyn Module>>, ModuleError> + Send + Sync,
>;

/// Default Discoverer implementation matching the Python/TS 8-stage pipeline.
///
/// Cross-language parity with apcore-python `Registry._discover_default`
/// and apcore-typescript `Registry._discoverDefault` (sync finding A-D-005).
///
/// # Construction
///
/// ```ignore
/// use std::sync::Arc;
/// use apcore::DefaultDiscoverer;
///
/// let factory = Arc::new(|file, entry_point: &str| {
///     // Look up `entry_point` in your application's module registry,
///     // construct an `Arc<dyn Module>`, and return it.
///     Ok(None)
/// });
///
/// let discoverer = DefaultDiscoverer::new()
///     .with_id_map(Some("path/to/id_map.yaml"))
///     .with_extensions(&[".toml"])
///     .with_factory(factory);
/// ```
pub struct DefaultDiscoverer {
    /// Optional path to an id_map.yaml file used at stage 2.
    id_map_path: Option<PathBuf>,
    /// File extensions considered as module candidates. Defaults to `[".rs"]`.
    extensions: Vec<String>,
    /// Maximum directory depth for filesystem scan.
    max_depth: u32,
    /// Whether to follow symlinks during scan.
    follow_symlinks: bool,
    /// User-provided factory that turns a discovered entry point into a
    /// live `Arc<dyn Module>`.
    factory: ModuleFactory,
}

impl std::fmt::Debug for DefaultDiscoverer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DefaultDiscoverer")
            .field("id_map_path", &self.id_map_path)
            .field("extensions", &self.extensions)
            .field("max_depth", &self.max_depth)
            .field("follow_symlinks", &self.follow_symlinks)
            .field("factory", &"<ModuleFactory>")
            .finish()
    }
}

impl DefaultDiscoverer {
    /// Create a new `DefaultDiscoverer` with a no-op factory that always
    /// returns `Ok(None)`. Use [`Self::with_factory`] to supply a real
    /// factory before passing this to a `Registry`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            id_map_path: None,
            extensions: vec![".rs".to_string()],
            max_depth: 8,
            follow_symlinks: false,
            factory: Arc::new(|_file, _entry| Ok(None)),
        }
    }

    /// Set the optional id_map.yaml path (stage 2 override mappings).
    #[must_use]
    pub fn with_id_map(mut self, path: Option<impl AsRef<Path>>) -> Self {
        self.id_map_path = path.map(|p| p.as_ref().to_path_buf());
        self
    }

    /// Override the default file extensions (default: `[".rs"]`).
    #[must_use]
    pub fn with_extensions(mut self, exts: &[&str]) -> Self {
        self.extensions = exts.iter().map(|s| (*s).to_string()).collect();
        self
    }

    /// Set the maximum directory depth for the filesystem scan (default: 8).
    #[must_use]
    pub fn with_max_depth(mut self, depth: u32) -> Self {
        self.max_depth = depth;
        self
    }

    /// Whether to follow symlinks during scanning (default: false).
    #[must_use]
    pub fn with_follow_symlinks(mut self, follow: bool) -> Self {
        self.follow_symlinks = follow;
        self
    }

    /// Set the module factory closure.
    #[must_use]
    pub fn with_factory(mut self, factory: ModuleFactory) -> Self {
        self.factory = factory;
        self
    }
}

impl Default for DefaultDiscoverer {
    fn default() -> Self {
        Self::new()
    }
}

/// Internal: discovery output for one file before the topo-sort stage.
struct Pending {
    file: DiscoveredFile,
    module: Arc<dyn Module>,
    descriptor: ModuleDescriptor,
    deps: Vec<DepInfo>,
}

#[async_trait]
impl Discoverer for DefaultDiscoverer {
    #[allow(clippy::too_many_lines)] // 8-stage pipeline naturally inlines stage steps
    async fn discover(&self, roots: &[String]) -> Result<Vec<DiscoveredModule>, ModuleError> {
        // Stage 1: scan every root recursively.
        let mut discovered_files: Vec<DiscoveredFile> = Vec::new();
        let ext_refs: Vec<&str> = self.extensions.iter().map(String::as_str).collect();
        for root in roots {
            let path = Path::new(root);
            // scan_extensions returns ConfigNotFoundError when the root is missing
            // — exactly the spec contract for discover().
            let mut files =
                scan_extensions(path, self.max_depth, self.follow_symlinks, Some(&ext_refs))?;
            discovered_files.append(&mut files);
        }

        // Stage 2: apply id_map overrides if configured.
        let id_overrides: HashMap<String, HashMap<String, serde_json::Value>> =
            match &self.id_map_path {
                Some(path) => load_id_map(path)?,
                None => HashMap::new(),
            };
        for file in &mut discovered_files {
            if let Some(override_entry) =
                id_overrides.get(file.file_path.to_string_lossy().as_ref())
            {
                if let Some(new_id) = override_entry.get("id").and_then(|v| v.as_str()) {
                    file.canonical_id = new_id.to_string();
                }
            }
        }

        // Stage 3: load companion `*_meta.yaml` for each file.
        let mut metadata_per_file: HashMap<PathBuf, HashMap<String, serde_json::Value>> =
            HashMap::new();
        for file in &discovered_files {
            if let Some(meta_path) = &file.meta_path {
                let meta = load_metadata(meta_path)?;
                metadata_per_file.insert(file.file_path.clone(), meta);
            }
        }

        // Stage 4 + 5: resolve entry-point name, validate, and instantiate via
        // the user-provided factory. We collect (canonical_id, descriptor,
        // module, deps) here; the topo-sort runs in stage 6.
        let mut pending: Vec<Pending> = Vec::new();
        for file in discovered_files {
            // Reserved-word check on the canonical_id (first segment).
            let first_seg = file.canonical_id.split('.').next().unwrap_or("");
            if RESERVED_WORDS.contains(&first_seg) {
                tracing::warn!(
                    canonical_id = %file.canonical_id,
                    "Skipping discovered file: first segment is a reserved word"
                );
                continue;
            }
            if file.canonical_id.len() > MAX_MODULE_ID_LENGTH {
                tracing::warn!(
                    canonical_id = %file.canonical_id,
                    "Skipping discovered file: module_id exceeds {MAX_MODULE_ID_LENGTH} chars"
                );
                continue;
            }

            // Stage 4: resolve entry-point name.
            let meta = metadata_per_file.get(&file.file_path);
            let entry_point_name =
                crate::registry::entry_point::resolve_entry_point_name(&file.file_path, meta)?
                    .unwrap_or_else(|| {
                        crate::registry::entry_point::infer_struct_name(&file.file_path)
                    });

            // Stage 4 (continued): instantiate via factory.
            let Some(module) = (self.factory)(&file, &entry_point_name)? else {
                tracing::debug!(
                    canonical_id = %file.canonical_id,
                    entry_point = %entry_point_name,
                    "DefaultDiscoverer factory returned None — skipping"
                );
                continue;
            };

            // Stage 5: build the descriptor from module + metadata (YAML wins).
            let descriptor = build_descriptor(&file, module.as_ref(), meta);

            // Parse declared dependencies for stage 6.
            let deps = if let Some(m) = meta {
                m.get("dependencies")
                    .and_then(|v| v.as_array())
                    .map(|arr| parse_dependencies(arr))
                    .unwrap_or_default()
            } else {
                Vec::new()
            };

            pending.push(Pending {
                file,
                module,
                descriptor,
                deps,
            });
        }

        // Stage 7: detect id conflicts (case-insensitive duplicates).
        let mut seen_ids_lower: HashSet<String> = HashSet::new();
        for p in &pending {
            let lower = p.file.canonical_id.to_lowercase();
            if !seen_ids_lower.insert(lower.clone()) {
                return Err(ModuleError::new(
                    ErrorCode::ModuleIdConflict,
                    format!(
                        "Duplicate module ID '{}' (case-insensitive) discovered in roots {:?}",
                        p.file.canonical_id, roots,
                    ),
                ));
            }
        }

        // Stage 6: dependency topo sort. resolve_dependencies surfaces
        // CircularDependencyError when Kahn's queue empties before all nodes
        // are processed — matches the spec contract for discover().
        let modules_with_deps: Vec<(String, Vec<DepInfo>)> = pending
            .iter()
            .map(|p| (p.file.canonical_id.clone(), p.deps.clone()))
            .collect();
        let module_versions: HashMap<String, String> = pending
            .iter()
            .map(|p| (p.file.canonical_id.clone(), p.descriptor.version.clone()))
            .collect();
        let load_order = resolve_dependencies(&modules_with_deps, None, Some(&module_versions))?;

        // Stage 8: produce DiscoveredModule entries in dependency order.
        let by_id: HashMap<String, Pending> = pending
            .into_iter()
            .map(|p| (p.file.canonical_id.clone(), p))
            .collect();

        let mut result = Vec::with_capacity(load_order.len());
        for id in load_order {
            if let Some(p) = by_id.get(&id) {
                result.push(DiscoveredModule {
                    name: p.file.canonical_id.clone(),
                    source: p.file.file_path.to_string_lossy().into_owned(),
                    descriptor: p.descriptor.clone(),
                    module: Arc::clone(&p.module),
                });
            }
        }
        Ok(result)
    }
}

/// Build a `ModuleDescriptor` from the live module + optional YAML metadata.
fn build_descriptor(
    file: &DiscoveredFile,
    module: &dyn Module,
    meta: Option<&HashMap<String, serde_json::Value>>,
) -> ModuleDescriptor {
    let yaml = meta.cloned().unwrap_or_default();

    let description = yaml
        .get("description")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map_or_else(|| module.description().to_string(), str::to_string);

    let version = yaml
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("1.0.0")
        .to_string();

    let tags: Vec<String> = yaml
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let documentation = yaml
        .get("documentation")
        .and_then(|v| v.as_str())
        .map(String::from);

    ModuleDescriptor {
        module_id: file.canonical_id.clone(),
        name: yaml.get("name").and_then(|v| v.as_str()).map(String::from),
        description,
        documentation,
        input_schema: module.input_schema(),
        output_schema: module.output_schema(),
        version,
        tags,
        annotations: None,
        examples: vec![],
        metadata: yaml,
        display: None,
        sunset_date: None,
        dependencies: vec![],
        enabled: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::Context;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::tempdir;

    #[derive(Debug)]
    struct TestModule {
        desc: String,
    }

    #[async_trait]
    impl Module for TestModule {
        fn description(&self) -> &str {
            &self.desc
        }
        fn input_schema(&self) -> serde_json::Value {
            json!({ "type": "object" })
        }
        fn output_schema(&self) -> serde_json::Value {
            json!({ "type": "object" })
        }
        async fn execute(
            &self,
            _inputs: serde_json::Value,
            _ctx: &Context<serde_json::Value>,
        ) -> Result<serde_json::Value, ModuleError> {
            Ok(json!({}))
        }
    }

    #[tokio::test]
    async fn missing_root_yields_config_not_found() {
        let discoverer = DefaultDiscoverer::new();
        let result = discoverer
            .discover(&["/this/path/does/not/exist".to_string()])
            .await;
        let err = result.expect_err("missing root should error");
        assert_eq!(err.code, ErrorCode::ConfigNotFound);
    }

    #[tokio::test]
    async fn empty_root_yields_empty_discovery() {
        let tmp = tempdir().unwrap();
        let discoverer = DefaultDiscoverer::new();
        let result = discoverer
            .discover(&[tmp.path().to_string_lossy().into_owned()])
            .await
            .unwrap();
        assert!(result.is_empty(), "no .rs files → no discovered modules");
    }

    #[tokio::test]
    async fn factory_invoked_for_discovered_file() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("hello.rs"), "// stub").unwrap();

        let calls = Arc::new(AtomicUsize::new(0));
        let calls_clone = Arc::clone(&calls);
        let factory: ModuleFactory = Arc::new(move |_file, entry_point| {
            calls_clone.fetch_add(1, Ordering::SeqCst);
            assert_eq!(entry_point, "Hello");
            Ok(Some(Arc::new(TestModule {
                desc: "test".to_string(),
            }) as Arc<dyn Module>))
        });

        let discoverer = DefaultDiscoverer::new().with_factory(factory);
        let result = discoverer
            .discover(&[tmp.path().to_string_lossy().into_owned()])
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "hello");
    }

    #[tokio::test]
    async fn circular_dependency_yields_dependency_error() {
        // Two files with deps on each other (a → b, b → a). We need to
        // construct a metadata file with a "dependencies" entry to trigger
        // the cycle detection in resolve_dependencies.
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "// a").unwrap();
        std::fs::write(
            tmp.path().join("a_meta.yaml"),
            "dependencies:\n  - module_id: b\n",
        )
        .unwrap();
        std::fs::write(tmp.path().join("b.rs"), "// b").unwrap();
        std::fs::write(
            tmp.path().join("b_meta.yaml"),
            "dependencies:\n  - module_id: a\n",
        )
        .unwrap();

        let factory: ModuleFactory = Arc::new(|_file, _entry| {
            Ok(Some(Arc::new(TestModule {
                desc: "circular".to_string(),
            }) as Arc<dyn Module>))
        });

        let discoverer = DefaultDiscoverer::new().with_factory(factory);
        let err = discoverer
            .discover(&[tmp.path().to_string_lossy().into_owned()])
            .await
            .expect_err("cycle should error");
        // resolve_dependencies surfaces cycles via CircularDependency code
        // when Kahn's queue empties before all nodes are processed.
        assert!(
            matches!(
                err.code,
                ErrorCode::CircularDependency | ErrorCode::DependencyNotFound
            ),
            "expected CircularDependency or DependencyNotFound, got {:?}",
            err.code
        );
    }
}

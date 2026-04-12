// APCore Protocol — Directory scanner for discovering extension modules
// Spec reference: Module discovery via filesystem scanning

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::errors::{ErrorCode, ModuleError};
use crate::registry::types::DiscoveredFile;

/// Directory names to skip during scanning.
const SKIP_DIR_NAMES: &[&str] = &["__pycache__", "node_modules", "target", ".git"];

/// File suffixes to skip during scanning.
const SKIP_FILE_SUFFIXES: &[&str] = &[".pyc", ".pyo"];

/// Default file extensions considered as module source files.
///
/// In Rust-centric discovery this would typically be `[".rs"]`, but the
/// scanner is designed to be language-agnostic so callers may override.
const DEFAULT_MODULE_EXTENSIONS: &[&str] = &[".rs"];

/// Recursively scan an extensions directory for module files.
///
/// Aligned with `apcore-python.scan_extensions` and
/// `apcore-typescript.scanExtensions`.
pub fn scan_extensions(
    root: &Path,
    max_depth: u32,
    follow_symlinks: bool,
    extensions: Option<&[&str]>,
) -> Result<Vec<DiscoveredFile>, ModuleError> {
    let root = root.canonicalize().map_err(|e| {
        ModuleError::new(
            ErrorCode::ConfigNotFound,
            format!("Extensions root not found: {} ({})", root.display(), e),
        )
    })?;

    let ext_list = extensions.unwrap_or(DEFAULT_MODULE_EXTENSIONS);
    let mut results: Vec<DiscoveredFile> = Vec::new();
    let mut seen_ids: HashMap<String, PathBuf> = HashMap::new();
    let mut seen_ids_lower: HashMap<String, String> = HashMap::new();
    let mut visited_real_paths: HashSet<PathBuf> = HashSet::new();
    visited_real_paths.insert(root.clone());

    scan_dir(
        &root,
        &root,
        1,
        max_depth,
        follow_symlinks,
        ext_list,
        &mut results,
        &mut seen_ids,
        &mut seen_ids_lower,
        &mut visited_real_paths,
    );

    Ok(results)
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)] // complex filesystem traversal with symlink/duplicate/case checks
fn scan_dir(
    root: &Path,
    dir_path: &Path,
    depth: u32,
    max_depth: u32,
    follow_symlinks: bool,
    extensions: &[&str],
    results: &mut Vec<DiscoveredFile>,
    seen_ids: &mut HashMap<String, PathBuf>,
    seen_ids_lower: &mut HashMap<String, String>,
    visited_real_paths: &mut HashSet<PathBuf>,
) {
    if depth > max_depth {
        tracing::info!(
            "Max depth {} exceeded at {}, skipping",
            max_depth,
            dir_path.display()
        );
        return;
    }

    let entries = match std::fs::read_dir(dir_path) {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("Error scanning directory {}: {}", dir_path.display(), e);
            return;
        }
    };

    for entry_result in entries {
        let entry = match entry_result {
            Ok(e) => e,
            Err(e) => {
                tracing::error!("Error reading directory entry: {}", e);
                continue;
            }
        };

        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Skip hidden and private entries
        if name_str.starts_with('.') || name_str.starts_with('_') {
            continue;
        }
        if SKIP_DIR_NAMES.contains(&name_str.as_ref()) {
            continue;
        }

        let entry_path = entry.path();
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(e) => {
                tracing::error!("Error accessing {}: {}", entry_path.display(), e);
                continue;
            }
        };

        if file_type.is_dir() || (file_type.is_symlink() && follow_symlinks) {
            if file_type.is_symlink() {
                if !follow_symlinks {
                    continue;
                }
                match entry_path.canonicalize() {
                    Ok(real) => {
                        if visited_real_paths.contains(&real) {
                            tracing::warn!(
                                "Symlink cycle detected at {} -> {}, skipping",
                                entry_path.display(),
                                real.display()
                            );
                            continue;
                        }
                        visited_real_paths.insert(real);
                    }
                    Err(_) => continue,
                }
            }
            if entry_path.is_dir() {
                scan_dir(
                    root,
                    &entry_path,
                    depth + 1,
                    max_depth,
                    follow_symlinks,
                    extensions,
                    results,
                    seen_ids,
                    seen_ids_lower,
                    visited_real_paths,
                );
            }
        } else if file_type.is_file() {
            let ext = entry_path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| format!(".{e}"));

            // Check skip suffixes
            if let Some(ref ext_str) = ext {
                if SKIP_FILE_SUFFIXES.contains(&ext_str.as_str()) {
                    continue;
                }
            }

            // Check allowed extensions
            let matches_ext = match &ext {
                Some(e) => extensions.contains(&e.as_str()),
                None => false,
            };
            if !matches_ext {
                continue;
            }

            // Derive canonical ID from relative path
            let Ok(rel) = entry_path.strip_prefix(root) else {
                continue;
            };
            let canonical_id = rel
                .with_extension("")
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, ".");

            // Duplicate check
            if let Some(existing_path) = seen_ids.get(&canonical_id) {
                tracing::error!(
                    "Duplicate module ID '{}' at {}, already found at {}. Skipping.",
                    canonical_id,
                    entry_path.display(),
                    existing_path.display()
                );
                continue;
            }

            // Case collision warning
            let lower_id = canonical_id.to_lowercase();
            if let Some(existing_id) = seen_ids_lower.get(&lower_id) {
                if *existing_id != canonical_id {
                    tracing::warn!(
                        "Case collision: '{}' and '{}' differ only by case",
                        canonical_id,
                        existing_id
                    );
                }
            }

            // Check for companion _meta.yaml
            let meta_path = entry_path.with_file_name(format!(
                "{}_meta.yaml",
                entry_path.file_stem().unwrap_or_default().to_string_lossy()
            ));
            let meta_path = if meta_path.exists() {
                Some(meta_path)
            } else {
                None
            };

            seen_ids.insert(canonical_id.clone(), entry_path.clone());
            seen_ids_lower.insert(lower_id, canonical_id.clone());

            results.push(DiscoveredFile {
                file_path: entry_path,
                canonical_id,
                meta_path,
                namespace: None,
            });
        }
    }
}

/// Scan multiple extension roots with namespace prefixing.
///
/// Each entry in `roots` must have a `"root"` key (path string) and an
/// optional `"namespace"` key (defaults to the directory name).
///
/// Aligned with `apcore-python.scan_multi_root` and
/// `apcore-typescript.scanMultiRoot`.
pub fn scan_multi_root<S: std::hash::BuildHasher>(
    roots: &[HashMap<String, String, S>],
    max_depth: u32,
    follow_symlinks: bool,
    extensions: Option<&[&str]>,
) -> Result<Vec<DiscoveredFile>, ModuleError> {
    let mut all_results: Vec<DiscoveredFile> = Vec::new();
    let mut seen_namespaces: HashSet<String> = HashSet::new();

    // Validate all namespaces before scanning
    let mut resolved: Vec<(PathBuf, String)> = Vec::new();
    for entry in roots {
        let root_str = entry.get("root").ok_or_else(|| {
            ModuleError::new(
                ErrorCode::ConfigInvalid,
                "Multi-root entry missing 'root' key",
            )
        })?;
        let root_path = PathBuf::from(root_str);
        let namespace = entry.get("namespace").cloned().unwrap_or_else(|| {
            root_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string()
        });
        if seen_namespaces.contains(&namespace) {
            return Err(ModuleError::new(
                ErrorCode::ConfigInvalid,
                format!("Duplicate namespace: '{namespace}'"),
            ));
        }
        seen_namespaces.insert(namespace.clone());
        resolved.push((root_path, namespace));
    }

    for (root_path, namespace) in resolved {
        let modules = scan_extensions(&root_path, max_depth, follow_symlinks, extensions)?;
        for m in modules {
            all_results.push(DiscoveredFile {
                file_path: m.file_path,
                canonical_id: format!("{}.{}", namespace, m.canonical_id),
                meta_path: m.meta_path,
                namespace: Some(namespace.clone()),
            });
        }
    }

    Ok(all_results)
}

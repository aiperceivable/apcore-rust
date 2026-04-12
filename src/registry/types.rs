// APCore Protocol — Registry types
// Spec reference: ModuleDescriptor, DiscoveredModule, DependencyInfo

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::module::{ModuleAnnotations, ModuleExample};

/// Cross-language compatible module descriptor.
///
/// Aligned with `apcore-python.ModuleDescriptor` and
/// `apcore-typescript.ModuleDescriptor`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FullModuleDescriptor {
    pub module_id: String,
    pub name: Option<String>,
    pub description: String,
    pub documentation: Option<String>,
    pub input_schema: serde_json::Value,
    pub output_schema: serde_json::Value,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub annotations: Option<ModuleAnnotations>,
    #[serde(default)]
    pub examples: Vec<ModuleExample>,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
    /// ISO 8601 date string (YYYY-MM-DD) after which this module is removed.
    #[serde(default)]
    pub sunset_date: Option<String>,
}

fn default_version() -> String {
    "1.0.0".to_string()
}

/// Intermediate representation of a discovered module file.
///
/// Aligned with `apcore-python.DiscoveredModule` and
/// `apcore-typescript.DiscoveredModule`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredFile {
    pub file_path: PathBuf,
    pub canonical_id: String,
    #[serde(default)]
    pub meta_path: Option<PathBuf>,
    #[serde(default)]
    pub namespace: Option<String>,
}

/// Parsed dependency information from module metadata.
///
/// Aligned with `apcore-python.DependencyInfo` and
/// `apcore-typescript.DependencyInfo`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepInfo {
    pub module_id: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub optional: bool,
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // -------------------------------------------------------------------------
    // FullModuleDescriptor
    // -------------------------------------------------------------------------

    #[test]
    fn full_module_descriptor_serializes_and_deserializes() {
        let desc = FullModuleDescriptor {
            module_id: "math.add".to_string(),
            name: Some("Add".to_string()),
            description: "Adds two numbers".to_string(),
            documentation: Some("## Details\nAdds a and b.".to_string()),
            input_schema: serde_json::json!({"type": "object"}),
            output_schema: serde_json::json!({"type": "object"}),
            version: "1.2.3".to_string(),
            tags: vec!["math".to_string()],
            annotations: None,
            examples: vec![],
            metadata: std::collections::HashMap::new(),
            sunset_date: None,
        };

        let json = serde_json::to_string(&desc).expect("should serialize");
        let restored: FullModuleDescriptor =
            serde_json::from_str(&json).expect("should deserialize");

        assert_eq!(restored.module_id, "math.add");
        assert_eq!(restored.version, "1.2.3");
        assert_eq!(restored.tags, vec!["math"]);
    }

    #[test]
    fn full_module_descriptor_default_version_is_1_0_0() {
        // When `version` is absent from JSON, it should default to "1.0.0"
        let json_str = r#"{
            "module_id": "test.module",
            "description": "A test module",
            "input_schema": {},
            "output_schema": {}
        }"#;
        let desc: FullModuleDescriptor =
            serde_json::from_str(json_str).expect("should deserialize");
        assert_eq!(desc.version, "1.0.0");
    }

    #[test]
    fn full_module_descriptor_optional_fields_default_to_none_or_empty() {
        let json_str = r#"{
            "module_id": "test.module",
            "description": "A test",
            "input_schema": {},
            "output_schema": {}
        }"#;
        let desc: FullModuleDescriptor =
            serde_json::from_str(json_str).expect("should deserialize");
        assert!(desc.name.is_none());
        assert!(desc.documentation.is_none());
        assert!(desc.annotations.is_none());
        assert!(desc.examples.is_empty());
        assert!(desc.metadata.is_empty());
        assert!(desc.sunset_date.is_none());
    }

    // -------------------------------------------------------------------------
    // DiscoveredFile
    // -------------------------------------------------------------------------

    #[test]
    fn discovered_file_serializes_and_deserializes() {
        let df = DiscoveredFile {
            file_path: PathBuf::from("/modules/math/add.rs"),
            canonical_id: "math.add".to_string(),
            meta_path: Some(PathBuf::from("/modules/math/add_meta.yaml")),
            namespace: Some("math".to_string()),
        };

        let json = serde_json::to_string(&df).expect("should serialize");
        let restored: DiscoveredFile = serde_json::from_str(&json).expect("should deserialize");

        assert_eq!(restored.canonical_id, "math.add");
        assert_eq!(restored.namespace.as_deref(), Some("math"));
        assert!(restored.meta_path.is_some());
    }

    #[test]
    fn discovered_file_optional_fields_default_correctly() {
        // meta_path and namespace default to None when absent
        let json_str = r#"{
            "file_path": "/modules/add.rs",
            "canonical_id": "add"
        }"#;
        let df: DiscoveredFile = serde_json::from_str(json_str).expect("should deserialize");
        assert!(df.meta_path.is_none());
        assert!(df.namespace.is_none());
    }

    // -------------------------------------------------------------------------
    // DepInfo
    // -------------------------------------------------------------------------

    #[test]
    fn dep_info_serializes_and_deserializes() {
        let dep = DepInfo {
            module_id: "email.smtp".to_string(),
            version: Some("^2.0.0".to_string()),
            optional: false,
        };
        let json = serde_json::to_string(&dep).expect("should serialize");
        let restored: DepInfo = serde_json::from_str(&json).expect("should deserialize");
        assert_eq!(restored.module_id, "email.smtp");
        assert_eq!(restored.version.as_deref(), Some("^2.0.0"));
        assert!(!restored.optional);
    }

    #[test]
    fn dep_info_optional_field_defaults_to_false() {
        let json_str = r#"{"module_id": "some.dep"}"#;
        let dep: DepInfo = serde_json::from_str(json_str).expect("should deserialize");
        assert!(!dep.optional);
        assert!(dep.version.is_none());
    }

    #[test]
    fn dep_info_clone_produces_equal_value() {
        let dep = DepInfo {
            module_id: "a.b".to_string(),
            version: None,
            optional: true,
        };
        let cloned = dep.clone();
        assert_eq!(cloned.module_id, dep.module_id);
        assert_eq!(cloned.optional, dep.optional);
    }
}

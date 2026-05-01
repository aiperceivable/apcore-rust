// APCore Protocol — Runtime config & toggle override persistence
// Spec reference: system-modules.md §1.1 Config and Feature Toggle Persistence (Issue #45)

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;

use crate::config::Config;
use crate::sys_modules::ToggleState;

/// Load YAML overrides into `config` and `toggle_state`.
///
/// Each top-level YAML key is treated as a dot-path config key. Keys prefixed
/// with `toggle.` are routed to `toggle_state` (boolean payload required).
/// Missing files are silently ignored — overrides are an opt-in layer applied
/// after the base config so a manual restore of the base never erases them.
pub fn load_overrides(
    overrides_path: &Path,
    config: &mut Config,
    toggle_state: Option<&ToggleState>,
) {
    if !overrides_path.exists() {
        return;
    }

    let raw = match std::fs::read_to_string(overrides_path) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                error = %e,
                path = %overrides_path.display(),
                "Failed to read overrides file; skipping"
            );
            return;
        }
    };

    let parsed: serde_yaml_ng::Value = match serde_yaml_ng::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                error = %e,
                path = %overrides_path.display(),
                "Overrides file is not valid YAML; skipping"
            );
            return;
        }
    };

    let serde_yaml_ng::Value::Mapping(map) = parsed else {
        tracing::warn!(
            path = %overrides_path.display(),
            "Overrides file root is not a mapping; skipping"
        );
        return;
    };

    let mut config_count = 0usize;
    let mut toggle_count = 0usize;
    for (k, v) in map {
        let Some(key) = k.as_str() else { continue };
        let json_value: serde_json::Value = match yaml_to_json(&v) {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!(error = %e, key = %key, "Skipping non-JSON-serializable override");
                continue;
            }
        };

        if let Some(module_id) = key.strip_prefix("toggle.") {
            if let (Some(ts), Some(enabled)) = (toggle_state, json_value.as_bool()) {
                if enabled {
                    ts.enable(module_id);
                } else {
                    ts.disable(module_id);
                }
                toggle_count += 1;
            }
        } else if !key.starts_with('_') {
            config.set(key, json_value);
            config_count += 1;
        }
    }

    tracing::info!(
        config_count,
        toggle_count,
        path = %overrides_path.display(),
        "Loaded overrides"
    );
}

/// Per-path lock registry — prevents concurrent writers from corrupting the
/// overrides file when multiple modules persist simultaneously.
fn write_lock_for(path: &Path) -> Arc<Mutex<()>> {
    use std::collections::HashMap;
    use std::sync::OnceLock;

    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
    let registry = LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = registry.lock();
    Arc::clone(
        guard
            .entry(path.to_path_buf())
            .or_insert_with(|| Arc::new(Mutex::new(()))),
    )
}

/// Persist a single key/value into the overrides YAML file.
///
/// The write is serialized per path and uses a tempfile + rename so a crashed
/// or interrupted writer cannot leave a half-written file behind.
pub fn write_override(overrides_path: &Path, key: &str, value: &serde_json::Value) {
    let lock = write_lock_for(overrides_path);
    let _g = lock.lock();

    if let Some(parent) = overrides_path.parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::error!(
                    error = %e,
                    path = %overrides_path.display(),
                    "Failed to create overrides parent directory"
                );
                return;
            }
        }
    }

    // Read existing overrides as an ordered mapping so the on-disk layout
    // remains stable across writes (BTreeMap → alphabetical key order).
    let mut existing: BTreeMap<String, serde_json::Value> =
        match std::fs::read_to_string(overrides_path) {
            Ok(s) if !s.is_empty() => match serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&s) {
                Ok(serde_yaml_ng::Value::Mapping(map)) => map
                    .into_iter()
                    .filter_map(|(k, v)| {
                        let k = k.as_str()?.to_string();
                        let v = yaml_to_json(&v).ok()?;
                        Some((k, v))
                    })
                    .collect(),
                _ => BTreeMap::new(),
            },
            _ => BTreeMap::new(),
        };

    existing.insert(key.to_string(), value.clone());

    let yaml = match serde_yaml_ng::to_string(&existing) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, key = %key, "Failed to serialize overrides YAML");
            return;
        }
    };

    let dir = overrides_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map_or_else(|| Path::new(".").to_path_buf(), Path::to_path_buf);
    let pid = std::process::id();
    let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
    let tmp = dir.join(format!(
        ".{}.{pid}.{nanos}.tmp",
        overrides_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("overrides")
    ));

    if let Err(e) = std::fs::write(&tmp, &yaml) {
        tracing::error!(error = %e, path = %tmp.display(), "Failed to write overrides tempfile");
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, overrides_path) {
        tracing::error!(
            error = %e,
            path = %overrides_path.display(),
            "Failed to rename overrides tempfile into place"
        );
        // best-effort cleanup
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Convert a `serde_yaml_ng::Value` to its `serde_json::Value` equivalent.
///
/// YAML supports a few non-JSON shapes (mappings with non-string keys, raw
/// tags) that we reject up-front rather than silently coerce.
fn yaml_to_json(v: &serde_yaml_ng::Value) -> Result<serde_json::Value, String> {
    match v {
        serde_yaml_ng::Value::Null => Ok(serde_json::Value::Null),
        serde_yaml_ng::Value::Bool(b) => Ok(serde_json::Value::Bool(*b)),
        serde_yaml_ng::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(serde_json::Value::Number(i.into()))
            } else if let Some(u) = n.as_u64() {
                Ok(serde_json::Value::Number(u.into()))
            } else if let Some(f) = n.as_f64() {
                serde_json::Number::from_f64(f)
                    .map(serde_json::Value::Number)
                    .ok_or_else(|| "non-finite f64".to_string())
            } else {
                Err("unrepresentable YAML number".to_string())
            }
        }
        serde_yaml_ng::Value::String(s) => Ok(serde_json::Value::String(s.clone())),
        serde_yaml_ng::Value::Sequence(seq) => seq
            .iter()
            .map(yaml_to_json)
            .collect::<Result<Vec<_>, _>>()
            .map(serde_json::Value::Array),
        serde_yaml_ng::Value::Mapping(map) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in map {
                let key = k
                    .as_str()
                    .ok_or_else(|| "non-string YAML mapping key".to_string())?
                    .to_string();
                obj.insert(key, yaml_to_json(v)?);
            }
            Ok(serde_json::Value::Object(obj))
        }
        serde_yaml_ng::Value::Tagged(tagged) => yaml_to_json(&tagged.value),
    }
}

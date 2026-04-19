// APCore Protocol — Extension point system
// Spec reference: Registration, query, and wiring of custom extensions
// (discoverers, middleware, ACL providers, span exporters, module validators,
// and approval handlers) into the apcore runtime.

use std::collections::HashMap;

use crate::acl::ACL;
use crate::approval::ApprovalHandler;
use crate::errors::{ErrorCode, ModuleError};
use crate::executor::Executor;
use crate::middleware::base::Middleware;
use crate::observability::span::SpanExporter;
use crate::observability::tracing_middleware::TracingMiddleware;
use crate::registry::registry::{Discoverer, ModuleValidator, Registry};

// ---------------------------------------------------------------------------
// ExtensionPoint — describes a named slot where extensions can be registered
// ---------------------------------------------------------------------------

/// Describes a named slot where extensions can be registered.
#[derive(Debug, Clone)]
pub struct ExtensionPoint {
    /// Name of the extension point (e.g. "middleware").
    pub name: String,
    /// Human-readable description of what this point accepts.
    pub description: String,
    /// Whether multiple extensions can be registered at this point.
    pub multiple: bool,
}

// ---------------------------------------------------------------------------
// ExtensionKind — type-safe enum of extension instances
// ---------------------------------------------------------------------------

/// A type-safe wrapper for the different kinds of extensions that can be
/// registered. This replaces the Python/TypeScript `Any` approach with Rust
/// enums so the type system enforces correctness at compile time.
pub enum ExtensionKind {
    /// A custom module discovery strategy.
    Discoverer(Box<dyn Discoverer>),
    /// Execution middleware.
    Middleware(Box<dyn Middleware>),
    /// Access control provider.
    Acl(ACL),
    /// Tracing span exporter.
    SpanExporter(Box<dyn SpanExporter>),
    /// Custom module validation.
    ModuleValidator(Box<dyn ModuleValidator>),
    /// Approval handler for Step 4.5 gate.
    ApprovalHandler(Box<dyn ApprovalHandler>),
}

impl std::fmt::Debug for ExtensionKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExtensionKind::Discoverer(_) => write!(f, "ExtensionKind::Discoverer(...)"),
            ExtensionKind::Middleware(m) => write!(f, "ExtensionKind::Middleware({m:?})"),
            ExtensionKind::Acl(_) => write!(f, "ExtensionKind::Acl(...)"),
            ExtensionKind::SpanExporter(e) => {
                write!(f, "ExtensionKind::SpanExporter({e:?})")
            }
            ExtensionKind::ModuleValidator(_) => {
                write!(f, "ExtensionKind::ModuleValidator(...)")
            }
            ExtensionKind::ApprovalHandler(h) => {
                write!(f, "ExtensionKind::ApprovalHandler({h:?})")
            }
        }
    }
}

impl ExtensionKind {
    /// Return the extension point name this kind corresponds to.
    fn point_name(&self) -> &str {
        match self {
            ExtensionKind::Discoverer(_) => "discoverer",
            ExtensionKind::Middleware(_) => "middleware",
            ExtensionKind::Acl(_) => "acl",
            ExtensionKind::SpanExporter(_) => "span_exporter",
            ExtensionKind::ModuleValidator(_) => "module_validator",
            ExtensionKind::ApprovalHandler(_) => "approval_handler",
        }
    }
}

// ---------------------------------------------------------------------------
// Built-in extension points
// ---------------------------------------------------------------------------

fn built_in_points() -> HashMap<String, ExtensionPoint> {
    let mut points = HashMap::new();
    points.insert(
        "discoverer".to_string(),
        ExtensionPoint {
            name: "discoverer".to_string(),
            description: "Custom module discovery strategy".to_string(),
            multiple: false,
        },
    );
    points.insert(
        "middleware".to_string(),
        ExtensionPoint {
            name: "middleware".to_string(),
            description: "Execution middleware".to_string(),
            multiple: true,
        },
    );
    points.insert(
        "acl".to_string(),
        ExtensionPoint {
            name: "acl".to_string(),
            description: "Access control provider".to_string(),
            multiple: false,
        },
    );
    points.insert(
        "span_exporter".to_string(),
        ExtensionPoint {
            name: "span_exporter".to_string(),
            description: "Tracing span exporter".to_string(),
            multiple: true,
        },
    );
    points.insert(
        "module_validator".to_string(),
        ExtensionPoint {
            name: "module_validator".to_string(),
            description: "Custom module validation".to_string(),
            multiple: false,
        },
    );
    points.insert(
        "approval_handler".to_string(),
        ExtensionPoint {
            name: "approval_handler".to_string(),
            description: "Approval handler for Step 4.5 gate".to_string(),
            multiple: false,
        },
    );
    points
}

// ---------------------------------------------------------------------------
// ExtensionManager
// ---------------------------------------------------------------------------

/// Manages extension points and their registered implementations.
///
/// Pre-registers six built-in extension points: discoverer, middleware,
/// acl, `span_exporter`, `module_validator`, and `approval_handler`.
///
/// Extensions are registered as [`ExtensionKind`] variants, ensuring type
/// safety at compile time rather than relying on runtime `isinstance` checks
/// as in Python/TypeScript.
pub struct ExtensionManager {
    points: HashMap<String, ExtensionPoint>,
    extensions: HashMap<String, Vec<ExtensionKind>>,
}

impl std::fmt::Debug for ExtensionManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtensionManager")
            .field("points", &self.points.keys().collect::<Vec<_>>())
            .field(
                "extensions",
                &self
                    .extensions
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.len()))
                    .collect::<HashMap<_, _>>(),
            )
            .finish()
    }
}

impl Default for ExtensionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ExtensionManager {
    /// Create a new extension manager with the built-in extension points.
    #[must_use]
    pub fn new() -> Self {
        let points = built_in_points();
        let extensions: HashMap<String, Vec<ExtensionKind>> =
            points.keys().map(|k| (k.clone(), Vec::new())).collect();
        Self { points, extensions }
    }

    /// Register an extension for the given extension point.
    ///
    /// The `extension` must be an [`ExtensionKind`] variant whose internal
    /// point name matches `point_name`.
    ///
    /// # Errors
    ///
    /// Returns [`ModuleError`] if `point_name` is unknown or if the
    /// `ExtensionKind` variant does not match the requested point.
    pub fn register(
        &mut self,
        point_name: &str,
        extension: ExtensionKind,
    ) -> Result<(), ModuleError> {
        if !self.points.contains_key(point_name) {
            let mut available: Vec<&str> = self
                .points
                .keys()
                .map(std::string::String::as_str)
                .collect();
            available.sort_unstable();
            return Err(ModuleError::new(
                ErrorCode::GeneralInvalidInput,
                format!(
                    "Unknown extension point: '{}'. Available: {}",
                    point_name,
                    available.join(", ")
                ),
            ));
        }

        // Verify the ExtensionKind variant matches the requested point.
        if extension.point_name() != point_name {
            return Err(ModuleError::new(
                ErrorCode::GeneralInvalidInput,
                format!(
                    "Extension for '{}' must be an ExtensionKind::{} variant, got {:?}",
                    point_name,
                    Self::variant_name_for_point(point_name),
                    extension.point_name()
                ),
            ));
        }

        let point = &self.points[point_name];
        if point.multiple {
            // INVARIANT: new() pre-populates extensions[point_name] for every point in self.points;
            // the register() guard above ensures point_name is in self.points before reaching here.
            self.extensions.get_mut(point_name).unwrap().push(extension);
        } else {
            self.extensions
                .insert(point_name.to_string(), vec![extension]);
        }

        Ok(())
    }

    /// Return the count of extensions registered at the given point, or
    /// `None` if the point is unknown.
    pub fn count(&self, point_name: &str) -> Option<usize> {
        self.extensions.get(point_name).map(std::vec::Vec::len)
    }

    /// Return whether the given extension point has any registered extensions.
    ///
    /// # Errors
    ///
    /// Returns [`ModuleError`] if `point_name` is unknown.
    pub fn has(&self, point_name: &str) -> Result<bool, ModuleError> {
        match self.extensions.get(point_name) {
            Some(exts) => Ok(!exts.is_empty()),
            None => Err(ModuleError::new(
                ErrorCode::GeneralInvalidInput,
                format!("Unknown extension point: '{point_name}'"),
            )),
        }
    }

    /// Return all registered extension points.
    #[must_use]
    pub fn list_points(&self) -> Vec<ExtensionPoint> {
        self.points.values().cloned().collect()
    }

    /// Clear all extensions for the given point.
    ///
    /// # Errors
    ///
    /// Returns [`ModuleError`] if `point_name` is unknown.
    pub fn clear(&mut self, point_name: &str) -> Result<(), ModuleError> {
        match self.extensions.get_mut(point_name) {
            Some(exts) => {
                exts.clear();
                Ok(())
            }
            None => Err(ModuleError::new(
                ErrorCode::GeneralInvalidInput,
                format!("Unknown extension point: '{point_name}'"),
            )),
        }
    }

    /// Clear all extensions across all points.
    pub fn clear_all(&mut self) {
        for exts in self.extensions.values_mut() {
            exts.clear();
        }
    }

    /// Wire all registered extensions into the given registry and executor.
    ///
    /// Connections:
    /// - discoverer -> `registry.set_discoverer()`
    /// - `module_validator` -> `registry.set_validator()`
    /// - acl -> `executor.set_acl()`
    /// - `approval_handler` -> `executor.set_approval_handler()`
    /// - middleware -> `executor.use_middleware()` for each
    ///
    /// Note: `span_exporter` wiring is logged as a warning if no
    /// `TracingMiddleware` is found; the Rust `TracingMiddleware` does not
    /// currently expose a `set_exporter` method, so exporters must be provided
    /// at construction time.
    pub fn apply(
        &mut self,
        registry: &Registry,
        executor: &mut Executor,
    ) -> Result<(), ModuleError> {
        // Discoverer
        if let Some(ExtensionKind::Discoverer(d)) = self.take_single("discoverer") {
            registry.set_discoverer(d);
        }

        // Module validator
        if let Some(ExtensionKind::ModuleValidator(v)) = self.take_single("module_validator") {
            registry.set_validator(v);
        }

        // ACL
        if let Some(ExtensionKind::Acl(acl)) = self.take_single("acl") {
            executor.set_acl(acl);
        }

        // Approval handler
        if let Some(ExtensionKind::ApprovalHandler(h)) = self.take_single("approval_handler") {
            executor.set_approval_handler(h);
        }

        // Middleware — drain all entries
        let middlewares = self
            .extensions
            .get_mut("middleware")
            .map(std::mem::take)
            .unwrap_or_default();
        for ext in middlewares {
            if let ExtensionKind::Middleware(mw) = ext {
                executor.use_middleware(mw)?;
            }
        }

        // Span exporters: wrap in TracingMiddleware and add to executor pipeline.
        // Aligned with apcore-typescript: find existing TracingMiddleware or create one.
        let exporters: Vec<Box<dyn SpanExporter>> = self
            .extensions
            .get_mut("span_exporter")
            .map(std::mem::take)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|ext| {
                if let ExtensionKind::SpanExporter(e) = ext {
                    Some(e)
                } else {
                    None
                }
            })
            .collect();

        if !exporters.is_empty() {
            if exporters.len() > 1 {
                tracing::warn!(
                    "[apcore:extensions] {} span_exporters registered; \
                     only the first will be wired (composite exporter not yet supported)",
                    exporters.len()
                );
            }
            let exporter = exporters.into_iter().next().unwrap();
            executor.use_middleware(Box::new(TracingMiddleware::new(exporter)))?;
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Take the single (last-registered) extension from a non-multiple point,
    /// draining the vector.
    fn take_single(&mut self, point_name: &str) -> Option<ExtensionKind> {
        self.extensions
            .get_mut(point_name)
            .and_then(std::vec::Vec::pop)
    }

    /// Map a point name to the expected `ExtensionKind` variant name.
    fn variant_name_for_point(point_name: &str) -> &'static str {
        match point_name {
            "discoverer" => "Discoverer",
            "middleware" => "Middleware",
            "acl" => "Acl",
            "span_exporter" => "SpanExporter",
            "module_validator" => "ModuleValidator",
            "approval_handler" => "ApprovalHandler",
            _ => "Unknown",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_has_built_in_points() {
        let mgr = ExtensionManager::new();
        let points = mgr.list_points();
        assert_eq!(points.len(), 6);

        let names: Vec<String> = points.iter().map(|p| p.name.clone()).collect();
        assert!(names.contains(&"discoverer".to_string()));
        assert!(names.contains(&"middleware".to_string()));
        assert!(names.contains(&"acl".to_string()));
        assert!(names.contains(&"span_exporter".to_string()));
        assert!(names.contains(&"module_validator".to_string()));
        assert!(names.contains(&"approval_handler".to_string()));
    }

    #[test]
    fn test_register_unknown_point_errors() {
        let mut mgr = ExtensionManager::new();
        let result = mgr.register(
            "nonexistent",
            ExtensionKind::Acl(ACL::new(vec![], "deny", None)),
        );
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Unknown extension point"));
    }

    #[test]
    fn test_register_mismatched_kind_errors() {
        let mut mgr = ExtensionManager::new();
        // Try to register an ACL at the "middleware" point.
        let result = mgr.register(
            "middleware",
            ExtensionKind::Acl(ACL::new(vec![], "deny", None)),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_register_acl_replaces_previous() {
        let mut mgr = ExtensionManager::new();
        let acl1 = ACL::new(vec![], "deny", None);
        let acl2 = ACL::new(vec![], "deny", None);
        mgr.register("acl", ExtensionKind::Acl(acl1)).unwrap();
        assert_eq!(mgr.count("acl"), Some(1));
        mgr.register("acl", ExtensionKind::Acl(acl2)).unwrap();
        // Non-multiple: replaces previous.
        assert_eq!(mgr.count("acl"), Some(1));
    }

    #[test]
    fn test_has_and_clear() {
        let mut mgr = ExtensionManager::new();
        assert!(!mgr.has("acl").unwrap());
        mgr.register("acl", ExtensionKind::Acl(ACL::new(vec![], "deny", None)))
            .unwrap();
        assert!(mgr.has("acl").unwrap());
        mgr.clear("acl").unwrap();
        assert!(!mgr.has("acl").unwrap());
    }

    #[test]
    fn test_clear_all() {
        let mut mgr = ExtensionManager::new();
        mgr.register("acl", ExtensionKind::Acl(ACL::new(vec![], "deny", None)))
            .unwrap();
        mgr.clear_all();
        assert!(!mgr.has("acl").unwrap());
    }

    #[test]
    fn test_default_impl() {
        let mgr = ExtensionManager::default();
        assert_eq!(mgr.list_points().len(), 6);
    }

    #[test]
    fn test_debug_impl() {
        let mgr = ExtensionManager::new();
        let debug_str = format!("{mgr:?}");
        assert!(debug_str.contains("ExtensionManager"));
    }
}

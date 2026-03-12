// APCore Protocol — Registry module
// Spec reference: Module registry, discovery, and validation

#[allow(clippy::module_inception)]
pub mod registry;

pub use registry::{
    DependencyInfo, DiscoveredModule, Discoverer, ModuleDescriptor, ModuleValidator, Registry,
};

// APCore Protocol — Registry module
// Spec reference: Module registry, discovery, and validation

pub mod registry;

pub use registry::{
    DependencyInfo, DiscoveredModule, Discoverer, ModuleDescriptor, ModuleValidator, Registry,
};

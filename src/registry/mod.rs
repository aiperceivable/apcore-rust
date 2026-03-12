// APCore Protocol — Registry module
// Spec reference: Module registry, discovery, and validation

pub mod registry;

pub use registry::{
    DependencyInfo, Discoverer, DiscoveredModule, ModuleDescriptor, ModuleValidator, Registry,
};

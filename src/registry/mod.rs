// APCore Protocol — Registry module
// Spec reference: Module registry, discovery, and validation

#[allow(clippy::module_inception)]
pub mod registry;

pub use registry::{
    module_id_pattern, registry_events, DependencyInfo, DiscoveredModule, Discoverer,
    ModuleDescriptor, ModuleValidator, Registry, RegistryEvents, MAX_MODULE_ID_LENGTH,
    REGISTRY_EVENTS, RESERVED_WORDS,
};

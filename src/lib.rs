// APCore SDK for Rust — AI Partner Core protocol implementation
// Main library module — re-exports all public API

pub mod acl;
pub mod approval;
pub mod async_task;
pub mod bindings;
pub mod cancel;
pub mod client;
pub mod config;
pub mod context;
pub mod decorator;
pub mod errors;
pub mod events;
pub mod executor;
pub mod extensions;
pub mod middleware;
pub mod module;
pub mod observability;
pub mod registry;
pub mod schema;
pub mod trace_context;
pub mod utils;
pub mod version;

// Re-export primary types at crate root for convenience.
pub use client::APCore;
pub use config::Config;
pub use context::{Context, ContextFactory, Identity};
pub use errors::{ErrorCode, ModuleError};
pub use executor::Executor;
pub use module::Module;

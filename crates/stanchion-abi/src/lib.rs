//! Runtime-agnostic types shared by every plugin backend.
//!
//! Registries, hosts and backends (Lua, WASM, …) all speak these types,
//! so a plugin can move between runtimes without the *caller* noticing.
//! This crate is deliberately runtime-free: it links no Lua, no WASM
//! engine, nothing backend-specific. Each backend owns its native
//! conversions and maps its errors into [`RuntimeError`].

pub mod backend;
pub mod callback;
pub mod error;
pub mod limits;
pub mod load;
pub mod panics;
pub mod manifest;
pub mod report;
pub mod rocks;
pub mod runtime;
pub mod sanitize;
pub mod signature;
pub mod value;

pub use backend::{BackendRegistry, PluginBackend, PluginInstance};
pub use callback::{
    AllowList, Approval, CapabilityCall, CapabilityProvider, CapabilityRequest, Decision, Grant,
    HostSetup, Policy, Rules, OPTIONAL_KEY, approve_capabilities,
};
pub use error::{Error, Result, RuntimeError};
pub use limits::ResourceLimits;
pub use load::{GroupOutcome, LoadContext, LoadItem};
pub use manifest::{
    DependencySpec, DetailedDependency, MANIFEST_FILE, Manifest, PluginType, is_valid_name,
    validate_name,
};
pub use report::{AuditEntry, Failure, LoadReport, PluginInfo};
pub use runtime::{Reloaded, Runtime};
pub use sanitize::{MAX_LOG_MESSAGE, sanitize_log};
pub use value::Value;

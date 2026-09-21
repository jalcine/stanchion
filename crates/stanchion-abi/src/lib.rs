//! Runtime-agnostic types shared by every plugin backend.
//!
//! The Lua registry ([`stanchion-registry`]) and the WASM backend
//! ([`stanchion-wasm`]) both speak these types, so a plugin can move between
//! runtimes without the *caller* noticing. This crate is deliberately Lua-free:
//! the `lua` feature enables only the conversions that need `mlua`, so a
//! WASM-only build links no Lua at all.

pub mod backend;
pub mod error;
pub mod manifest;
pub mod value;

pub use backend::{BackendRegistry, PluginBackend, PluginInstance};
pub use error::{Error, Result};
pub use manifest::{
    DependencySpec, DetailedDependency, MANIFEST_FILE, Manifest, PluginType,
};
pub use value::Value;

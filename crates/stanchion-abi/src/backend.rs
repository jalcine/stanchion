//! Runtime-agnostic plugin backend interface.
//!
//! Every plugin runtime (Lua, WASM, etc.) implements [`PluginBackend`] and
//! [`PluginInstance`] so a host can load and call plugins without knowing which
//! engine produced them.

use std::path::Path;

use crate::error::Result;
use crate::manifest::{Manifest, PluginType};
use crate::value::Value;

/// A loaded plugin instance, callable by exported function/method name.
pub trait PluginInstance: Send + Sync {
    /// Calls a method (or exported function) on this plugin and returns the result.
    fn call(&self, method: &str, args: &[Value]) -> Result<Value>;

    /// A human-readable label for the runtime that produced this instance.
    fn runtime(&self) -> &str;
}

/// A factory for one kind of plugin runtime.
///
/// Each variant of [`PluginType`] has a corresponding backend that knows how
/// to compile and instantiate its artifacts from a plugin directory.
pub trait PluginBackend: Send + Sync {
    /// The [`PluginType`] this backend handles.
    fn plugin_type(&self) -> PluginType;

    /// Loads a plugin from its directory and returns an instance handle.
    fn load(&self, manifest: &Manifest, dir: &Path) -> Result<Box<dyn PluginInstance>>;
}

/// A registry of backends, keyed by [`PluginType`].
///
/// A host selects the right backend when loading a plugin based on the manifest's
/// `plugin_type` field. Multiple backends can coexist, but only the last
/// registration per type is kept (last-write-wins).
#[derive(Default)]
pub struct BackendRegistry {
    backends: Vec<Box<dyn PluginBackend>>,
}

impl BackendRegistry {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self {
            backends: Vec::new(),
        }
    }

    /// Registers a backend, replacing any previous one for the same plugin type.
    pub fn register(&mut self, backend: Box<dyn PluginBackend>) {
        let ty = backend.plugin_type();
        self.backends.retain(|b| b.plugin_type() != ty);
        self.backends.push(backend);
    }

    /// Finds a backend for the given plugin type, if one is registered.
    pub fn get(&self, ty: &PluginType) -> Option<&dyn PluginBackend> {
        self.backends
            .iter()
            .find(|b| b.plugin_type() == *ty)
            .map(|b| b.as_ref())
    }

    /// Returns `true` when no backends are registered.
    pub fn is_empty(&self) -> bool {
        self.backends.is_empty()
    }

    /// The number of registered backends.
    pub fn len(&self) -> usize {
        self.backends.len()
    }

    /// Iterates over all registered backends.
    pub fn iter(&self) -> impl Iterator<Item = &dyn PluginBackend> {
        self.backends.iter().map(|b| b.as_ref())
    }
}

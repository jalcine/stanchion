//! Runtime-agnostic plugin backend interface, re-exported from
//! [`stanchion_abi`], plus the built-in Lua backend.

pub use stanchion_abi::backend::{BackendRegistry, PluginBackend, PluginInstance};

use std::sync::Arc;
use tokio::sync::Mutex;

use crate::error::{Error, Result};
use crate::value::Value;

/// The built-in Lua backend, wrapping a `stanchion_lua` instance.
///
/// This is registered automatically by [`Stanchion`](crate::Stanchion) and
/// handles `plugin_type = "lua"` manifests.
pub struct LuaBackend {
    lua_runtime: Arc<dyn stanchion_abi::Runtime>,
}

impl LuaBackend {
    pub(crate) fn new(lua_runtime: Arc<dyn stanchion_abi::Runtime>) -> Self {
        Self { lua_runtime }
    }
}

impl PluginBackend for LuaBackend {
    fn plugin_type(&self) -> stanchion_abi::manifest::PluginType {
        stanchion_abi::manifest::PluginType::Lua
    }

    fn load(
        &self,
        manifest: &stanchion_abi::manifest::Manifest,
        dir: &std::path::Path,
    ) -> Result<Box<dyn PluginInstance>> {
        self.lua_runtime.load(manifest, dir)
    }
}
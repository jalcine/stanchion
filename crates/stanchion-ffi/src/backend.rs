//! Runtime-agnostic plugin backend interface, re-exported from
//! [`stanchion_abi`], plus the built-in Lua backend.

use std::sync::Arc;

use mlua::{Lua, MultiValue};
use stanchion_registry::DynInstance;
use tokio::sync::Mutex;

pub use stanchion_abi::backend::{BackendRegistry, PluginBackend, PluginInstance};

use crate::error::{Error, Result};
use crate::value::Value;

/// The built-in Lua backend, wrapping [`DynInstance`].
///
/// This is registered automatically by [`Stanchion`](crate::Stanchion) and
/// handles `plugin_type = "lua"` manifests.
pub struct LuaBackend {
    registry: Arc<Mutex<stanchion_registry::Registry<stanchion_registry::DynClass>>>,
}

impl LuaBackend {
    pub(crate) fn new(
        registry: Arc<Mutex<stanchion_registry::Registry<stanchion_registry::DynClass>>>,
    ) -> Self {
        Self { registry }
    }
}

impl PluginBackend for LuaBackend {
    fn plugin_type(&self) -> stanchion_abi::manifest::PluginType {
        stanchion_abi::manifest::PluginType::Lua
    }

    fn load(
        &self,
        manifest: &stanchion_abi::manifest::Manifest,
        _dir: &std::path::Path,
    ) -> Result<Box<dyn PluginInstance>> {
        let registry = futures_executor::block_on(self.registry.lock());
        let plugin = registry
            .get(&manifest.name)
            .ok_or_else(|| Error::UnknownPlugin(manifest.name.clone()))?;
        let instance = plugin.instance().clone();
        let lua = plugin.lua().clone();
        Ok(Box::new(LuaInstance {
            lua: Arc::new(lua),
            instance: Arc::new(instance),
        }))
    }
}

struct LuaInstance {
    lua: Arc<Lua>,
    instance: Arc<DynInstance>,
}

impl PluginInstance for LuaInstance {
    fn call(&self, method: &str, args: &[Value]) -> Result<Value> {
        let mut converted = Vec::with_capacity(args.len());
        for arg in args {
            converted.push(arg.to_lua(&self.lua)?);
        }
        let result = self.instance.call_method(method, MultiValue::from_iter(converted))?;
        Ok(Value::from_lua(&result)?)
    }

    fn runtime(&self) -> &str {
        "lua"
    }
}
//! Runtime-agnostic interface for plugin execution.
//!
//! This module re-exports the [`Runtime`] trait from [`stanchion_abi`].
//! Concrete runtime implementations live in their respective crates
//! (`stanchion-lua`, `stanchion-wasm`) and implement this trait.
//!
//! Additionally, this module provides a [`LuaRuntime`] wrapper for
//! `&mlua::Lua` to enable gradual migration to the runtime abstraction.

use std::io::Write;
use std::path::Path;

use mlua::{Lua, Value};
use stanchion_abi::{
    manifest::Manifest,
    runtime::Runtime,
    value::Value as AbiValue,
    PluginInstance,
};

/// A runtime wrapper for `Lua` that implements the [`Runtime`] trait.
    ///
    /// This enables gradual migration from direct `Lua` usage to the
    /// [`Runtime`] abstraction. The wrapper delegates Lua operations
    /// to the underlying `Lua` state.
    pub struct LuaRuntime {
        /// The Lua state to delegate to.
        lua: std::sync::Arc<std::sync::Mutex<mlua::Lua>>,
    }

    impl LuaRuntime {
        /// Creates a new `LuaRuntime` wrapper for the given Lua state.
        pub fn new(lua: mlua::Lua) -> Self {
            Self {
                lua: std::sync::Arc::new(std::sync::Mutex::new(lua)),
            }
        }
    }

    impl Runtime for LuaRuntime {
        fn load(
            &self,
            _manifest: &Manifest,
            _dir: &Path,
        ) -> stanchion_abi::Result<Box<dyn PluginInstance>> {
            // TODO: Implement proper plugin loading using LuaBackend.
            // For now, stub to allow compilation.
            unimplemented!("LuaRuntime::load - use LuaBackend for production")
        }

        fn verify(&self, _manifest: &Manifest, _dir: &Path) -> stanchion_abi::Result<()> {
            // TODO: Implement verification.
            unimplemented!("LuaRuntime::verify")
        }

        fn audit(&self, _log: &mut dyn Write) -> stanchion_abi::Result<()> {
            // TODO: Implement audit logging.
            unimplemented!("LuaRuntime::audit")
        }

        fn call(
            &self,
            _instance: &dyn PluginInstance,
            _method: &str,
            _args: &[AbiValue],
        ) -> stanchion_abi::Result<AbiValue> {
            // TODO: Implement method call on plugin instance.
            unimplemented!("LuaRuntime::call")
        }

        fn budget(&self, _plugin_name: &str) -> stanchion_abi::Result<u64> {
            // TODO: Implement budget tracking.
            Ok(u64::MAX)
        }

        fn reset_budget(&self, _plugin_name: &str) -> stanchion_abi::Result<()> {
            // TODO: Implement budget reset.
            Ok(())
        }

        fn runtime_name(&self) -> &'static str {
            "lua"
        }

        fn plugin_type(&self) -> stanchion_abi::PluginType {
            stanchion_abi::PluginType::Lua
        }

        fn install_capability(
            &self,
            _name: &str,
            _provider: &dyn stanchion_abi::callback::CapabilityProvider,
            _grant: &stanchion_abi::callback::Grant,
        ) -> stanchion_abi::Result<()> {
            // The capability installation is handled by the registry's
            // with_setup mechanism, which uses the Runtime trait to bind
            // functions into plugin environments.
            Ok(())
        }

        fn lua_state(&self) -> Option<std::sync::Arc<std::sync::Mutex<mlua::Lua>>> {
            Some(std::sync::Arc::clone(&self.lua))
        }
    }

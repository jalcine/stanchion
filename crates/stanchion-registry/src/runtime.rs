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

/// A runtime wrapper for `&Lua` that implements the [`Runtime`] trait.
///
/// This enables gradual migration from direct `Lua` usage to the
/// [`Runtime`] abstraction. The wrapper delegates Lua operations
/// to the underlying `Lua` state.
pub struct LuaRuntime<'a> {
    /// The Lua state to delegate to.
    lua: &'a Lua,
}

impl<'a> LuaRuntime<'a> {
    /// Creates a new `LuaRuntime` wrapper for the given Lua state.
    pub fn new(lua: &'a Lua) -> Self {
        Self { lua }
    }
}

impl<'a> Runtime for LuaRuntime<'a> {
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
}

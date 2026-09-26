//! Runtime-agnostic interface for plugin execution.
//!
//! Defines the [`Runtime`] trait that replaces direct `mlua::Lua`
//! usage inside `stanchion-registry`. Implemented by `stanchion-lua`
//! (via [`LuaBackend`]/[`LuaInstance`]) and `stanchion-wasm`.
//!
//! The registry holds a [`Box<dyn Runtime>`] and delegates all
//! runtime-specific operations (load, call, verify, audit, budget)
//! to it. This allows the registry to be runtime-agnostic while
//! the concrete runtime implementations live in their respective
//! crates.

use std::io::Write;
use std::path::Path;

use crate::backend::PluginInstance;
use crate::error::Result;
use crate::manifest::{Manifest, PluginType};
use crate::value::Value;

/// A runtime backend (Lua, Wasm, etc.) that can load, call, verify,
/// audit and enforce budgets for plugins.
///
/// This trait replaces `&Lua` parameters in registry functions,
/// enabling runtime-agnostic plugin management.
pub trait Runtime: Send + Sync {
    /// Loads a plugin from directory and returns an instance.
    fn load(&self, manifest: &Manifest, dir: &Path) -> Result<Box<dyn PluginInstance>>;

    /// Verifies plugin artifacts (manifest integrity, rocks, signatures).
    fn verify(&self, manifest: &Manifest, dir: &Path) -> Result<()>;

    /// Writes audit information to `log` (called before plugin execution).
    fn audit(&self, log: &mut dyn Write) -> Result<()>;

    /// Calls a method on a loaded plugin instance.
    fn call(&self, instance: &dyn PluginInstance, method: &str, args: &[Value]) -> Result<Value>;

    /// Gets remaining instruction budget for a plugin.
    fn budget(&self, plugin_name: &str) -> Result<u64>;

    /// Resets the instruction budget for a plugin.
    fn reset_budget(&self, plugin_name: &str) -> Result<()>;

    /// Human-readable runtime name (e.g. `"lua"`, `"wasm"`).
    fn runtime_name(&self) -> &'static str;

    /// The [`PluginType`] this runtime handles.
    fn plugin_type(&self) -> PluginType;

    /// Installs a capability function into the plugin environment.
    ///
    /// The runtime creates the appropriate callable (e.g. a Lua
    /// function) and binds it in each plugin's state.
    fn install_capability(
        &self,
        name: &str,
        provider: &dyn crate::callback::CapabilityProvider,
        grant: &crate::callback::Grant,
    ) -> Result<()>;

    /// Returns the underlying Lua state for Lua backends.
    /// Returns `None` for non-Lua backends.
    fn lua_state(&self) -> Option<std::sync::Arc<std::sync::Mutex<mlua::Lua>>> {
        None
    }
}

//! Runtime-agnostic interface for plugin execution.
//!
//! [`PluginBackend`](crate::backend::PluginBackend) loads one plugin.
//! [`Runtime`] loads and manages *groups* of plugins: dependency wiring,
//! shared states, reload and capability revocation all need visibility
//! beyond a single instance, so they live here. Hosts (e.g.
//! `stanchion-registry`) program against this trait and never touch a
//! backend's native types.
//!
//! Per-call budgets are enforced inside
//! [`PluginInstance::call`](crate::backend::PluginInstance::call), not
//! here: each call gets a fresh allowance, so a long-lived plugin never
//! exhausts a lifetime budget.

use crate::backend::PluginBackend;
use crate::error::Result;
use crate::load::{GroupOutcome, LoadContext, LoadItem};

/// A runtime backend (Lua, Wasm, etc.) managing the plugins it loaded.
///
/// One plugin failing never stops the others: group loads report per-plugin
/// outcomes, and reload leaves the old instance in place on failure.
pub trait Runtime: PluginBackend {
    /// Human-readable runtime name (e.g. `"lua"`, `"wasm"`).
    fn runtime_name(&self) -> &'static str;

    /// Instantiates a verified group, wiring dependencies between members.
    ///
    /// `items` arrive in dependency order with entry bytes already bound to
    /// their digests. Members that must share state (a dependency chain)
    /// do; failures are reported per plugin in the returned outcomes.
    fn load_group(&self, items: &[LoadItem], ctx: &LoadContext) -> Vec<GroupOutcome>;

    /// Re-reads one plugin, swapping in a fresh instance.
    ///
    /// On failure the old instance keeps serving; the error describes why
    /// the new one did not take its place.
    fn reload_plugin(
        &self,
        item: &LoadItem,
        ctx: &LoadContext,
    ) -> Result<Box<dyn crate::backend::PluginInstance>>;

    /// Unbinds a granted capability from a live instance.
    ///
    /// Returns whether the instance held it. Code that already captured the
    /// value keeps it, so this defangs a misbehaving plugin without
    /// rewinding it.
    fn revoke_capability(&self, instance: &dyn crate::backend::PluginInstance, capability: &str)
    -> bool;

    /// Forgets everything retained for `name`: states, proxies, budgets.
    ///
    /// Called after a plugin is removed so a later plugin reusing the name
    /// starts clean instead of resurrecting the old state.
    fn unload(&self, name: &str);
}

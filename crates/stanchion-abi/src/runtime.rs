//! Runtime-agnostic interface for plugin execution.
//!
//! [`PluginBackend`] loads one plugin.
//! [`Runtime`] loads and manages *groups* of plugins: dependency wiring,
//! shared states and reload all need visibility
//! beyond a single instance, so they live here. Hosts (e.g.
//! `stanchion-registry`) program against this trait and never touch a
//! backend's native types.
//!
//! Capability revocation is the exception: it belongs to the instance holding the
//! binding, so it lives on
//! [`PluginInstance::revoke_capability`](crate::backend::PluginInstance::revoke_capability).
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
///
/// Revocation is **not** here: it lives on
/// [`PluginInstance::revoke_capability`](crate::backend::PluginInstance::revoke_capability),
/// because the instance is what holds the binding. Routing it through the runtime
/// meant handing a backend a `&dyn PluginInstance` to downcast back to its own type.
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
    ///
    /// Returns what policy granted along with the instance, exactly as
    /// [`GroupOutcome::Loaded`] does. This used to
    /// return the instance alone, which left the host no way to learn the grants the
    /// backend had just computed — so `Registry::reload` recomputed them itself,
    /// consulting the policy a second time and deriving the same field by a different
    /// route than `load_dir` uses.
    fn reload_plugin(&self, item: &LoadItem, ctx: &LoadContext) -> Result<Reloaded>;

    /// Forgets everything retained for `name`: states, proxies, budgets.
    ///
    /// Called after a plugin is removed so a later plugin reusing the name
    /// starts clean instead of resurrecting the old state.
    fn unload(&self, name: &str);
}

/// A reloaded plugin: the fresh instance and what policy granted it.
pub struct Reloaded {
    /// The instance that replaces the old one.
    pub instance: Box<dyn crate::backend::PluginInstance>,
    /// Capabilities actually granted, after policy ran.
    pub granted: Vec<String>,
}

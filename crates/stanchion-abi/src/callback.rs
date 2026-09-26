//! Foreign-facing capability types.
//!
//! These types are shared between `stanchion-ffi` (which receives
//! foreign implementations) and `stanchion-lua`/`stanchion-wasm`
//! (which install them into plugin environments).

use crate::value::Value;

/// A capability provider, as seen by the runtime.
///
/// This is the foreign-facing trait; the runtime wraps it into a
/// plugin-callable function.
pub trait CapabilityProvider: Send + Sync {
    /// Does whatever the capability offers, and answers the plugin.
    fn invoke(&self, call: &CapabilityCall) -> std::result::Result<Value, String>;
}

/// What a plugin asked for, as a foreign policy sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct CapabilityRequest {
    /// Plugin making the request.
    pub plugin: String,
    /// Capability name.
    pub capability: String,
    /// Parameters declared in the plugin's manifest.
    pub params: Value,
    /// Whether the plugin still loads if this is denied.
    pub optional: bool,
}

/// A policy's answer to one request.
#[derive(Clone, Debug, PartialEq)]
pub enum Decision {
    /// Grant exactly what was asked for.
    Grant,
    /// Grant, but with these parameters instead of the requested ones.
    GrantWith(Value),
    /// Refuse, with a reason the plugin author will see.
    Deny(String),
}

/// A plugin reaching back into the host through a granted capability.
#[derive(Clone, Debug, PartialEq)]
pub struct CapabilityCall {
    /// Plugin making the call.
    pub plugin: String,
    /// Capability it was granted.
    pub capability: String,
    /// The parameters policy actually approved, which may be narrower than the
    /// manifest asked for.
    pub grant: Value,
    /// Arguments the plugin passed.
    pub args: Vec<Value>,
}

/// A grant, as seen by the runtime when installing a capability.
#[derive(Clone, Debug)]
pub struct Grant {
    /// Plugin this grant was issued to.
    pub plugin: String,
    /// Capability name.
    pub name: String,
    /// Approved parameters.
    pub params: Value,
}

impl Grant {
    /// Creates a new `Grant`.
    pub fn new(plugin: String, name: String, params: Value) -> Self {
        Self { plugin, name, params }
    }
}
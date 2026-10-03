//! What a host reports back about the plugins it loaded.
//!
//! These are the flattened, caller-facing shapes — the ones a binding returns and the
//! ones that travel over the remote protocol, as distinct from
//! `stanchion_registry::LoadReport`, which keeps the richer per-plugin detail
//! (directory paths, structured failure reasons) that only makes sense in-process.
//!
//! They lived twice: once in `stanchion-ffi` and once in `stanchion-remote`, with the
//! same fields and the same `is_clean`. Two vocabularies that have to mean the same
//! thing, and nothing making them. They had already drifted —
//! [`PluginInfo::runtime`] existed on the binding side and not on the wire, so a
//! remote host could not tell a caller which runtime had produced a plugin.
//!
//! Everything here is `Serialize`/`Deserialize` because the remote protocol sends it.
//! A new field gets `#[serde(default)]` so a host and a client at different versions
//! still understand each other.

use serde::{Deserialize, Serialize};

/// What one `load` call did.
///
/// One plugin failing never stops the others, so this reports both halves rather than
/// returning at the first problem.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LoadReport {
    /// Names of plugins that loaded, in the order they were constructed.
    pub loaded: Vec<String>,
    /// Plugins that did not, and why.
    pub failures: Vec<Failure>,
}

impl LoadReport {
    /// True when every discovered plugin loaded.
    pub fn is_clean(&self) -> bool {
        self.failures.is_empty()
    }
}

/// One plugin that did not load.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Failure {
    /// The plugin that failed, by name.
    pub plugin: String,
    /// Why, rendered for display.
    pub reason: String,
}

/// A loaded plugin, as a caller sees it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginInfo {
    /// Plugin name.
    pub name: String,
    /// Declared version, where the manifest carried one.
    pub version: Option<String>,
    /// Capabilities the policy granted.
    pub granted: Vec<String>,
    /// Who signed it, or why it counts as unsigned.
    pub signer: String,
    /// The runtime that produced the instance, e.g. `"lua"` or `"wasm"`.
    ///
    /// `#[serde(default)]` so a message from a host predating this field still
    /// deserializes, as an empty string rather than a failure.
    #[serde(default)]
    pub runtime: String,
}

/// What one plugin requests, established without executing any of its code.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuditEntry {
    /// The plugin that made the requests.
    pub plugin: String,
    /// Capabilities it declares.
    pub capabilities: Vec<String>,
    /// Who signed it, or why it counts as unsigned.
    pub signer: String,
}

//! JSON-RPC 2.0 protocol for the out-of-process plugin host.
//!
//! The channel is symmetric: the application calls the host to load and invoke
//! plugins, and the host calls the application when a plugin uses a capability the
//! application provides. That is precisely JSON-RPC's shape, so this speaks the real
//! thing rather than a bespoke framing — a host can be written in any language that
//! can read `Content-Length`-framed JSON on a pipe.
//!
//! [`lsp_server`] supplies the message types, framing and pending-request bookkeeping.
//! Nothing here is LSP-specific; that crate is simply the maintained, synchronous,
//! dependency-light JSON-RPC implementation for exactly this transport.

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

/// Methods the application calls on the host.
pub mod method {
    /// Discover and load every plugin under a root.
    pub const LOAD: &str = "plugins/load";
    /// Report the loaded plugins.
    pub const LIST: &str = "plugins/list";
    /// Report what plugins request, without running their code.
    pub const AUDIT: &str = "plugins/audit";
    /// Call one method on one plugin.
    pub const CALL: &str = "plugins/call";
    /// Call the same method on every plugin.
    pub const DISPATCH: &str = "plugins/dispatch";
    /// Re-read one plugin from disk.
    pub const RELOAD: &str = "plugins/reload";
    /// Unbind a capability from a live plugin.
    pub const REVOKE: &str = "plugins/revoke";
    /// Describe the host.
    pub const INFO: &str = "host/info";
    /// Finish serving and exit.
    pub const SHUTDOWN: &str = "host/shutdown";

    /// Prefix for host-to-application capability callbacks: `capability/<name>`.
    pub const CAPABILITY_PREFIX: &str = "capability/";
}

/// `plugins/load` and `plugins/audit`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RootParams {
    pub root: String,
}

/// `plugins/call`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallParams {
    pub plugin: String,
    pub method: String,
    #[serde(default)]
    pub args: Vec<Json>,
}

/// `plugins/dispatch`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchParams {
    pub method: String,
    #[serde(default)]
    pub args: Vec<Json>,
}

/// `plugins/reload`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginParams {
    pub plugin: String,
}

/// `plugins/revoke`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevokeParams {
    pub plugin: String,
    pub capability: String,
}

/// Result of `plugins/load`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LoadResult {
    /// Plugins that loaded, in order.
    pub loaded: Vec<String>,
    /// Plugins that did not, with the host's reason.
    pub failures: Vec<Failure>,
}

impl LoadResult {
    /// True when every discovered plugin loaded.
    pub fn is_clean(&self) -> bool {
        self.failures.is_empty()
    }
}

/// One plugin that did not load.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Failure {
    pub plugin: String,
    pub reason: String,
}

/// A loaded plugin, as the application sees it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    pub name: String,
    pub version: Option<String>,
    pub granted: Vec<String>,
    pub signer: String,
}

/// What one plugin requests, established without executing it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub plugin: String,
    pub capabilities: Vec<String>,
    pub signer: String,
}

/// One plugin's result from a dispatch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Outcome {
    pub plugin: String,
    /// Present when the call succeeded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Json>,
    /// Present when it failed. One plugin failing never affects the others.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Result of `host/info`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostInfo {
    pub version: String,
    pub isolation: String,
    pub signatures_required: bool,
}

/// A plugin reaching back into the application through a granted capability.
///
/// The approved grant travels with the call so the application can re-check it
/// rather than trusting the host to have narrowed correctly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallbackCall {
    /// Plugin making the call.
    pub plugin: String,
    /// Capability it was granted. Also the method name's suffix.
    pub capability: String,
    /// Parameters the host's policy approved for that grant.
    pub grant: Json,
    /// Arguments the plugin passed.
    pub args: Vec<Json>,
}

/// JSON-RPC error codes this protocol uses.
pub mod error_code {
    /// Reserved by JSON-RPC for a method the peer does not implement.
    pub const METHOD_NOT_FOUND: i32 = -32601;
    /// Reserved by JSON-RPC for malformed parameters.
    pub const INVALID_PARAMS: i32 = -32602;
    /// A plugin, or the host, failed while doing what was asked.
    pub const REQUEST_FAILED: i32 = -32000;
}

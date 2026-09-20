//! Running plugins in a separate process, spoken to over RPC.
//!
//! Everything else in this crate bounds what a plugin can *reach*. None of it bounds
//! what a plugin can do to the process it runs in: a sandbox cannot stop a segfault in
//! a C rock, and an instruction limit cannot save a state whose allocator already
//! failed. Moving plugins into a child process makes those failures survivable — the
//! child dies, the application notices, and nothing in its address space was at risk.
//!
//! [`RemoteRegistry`] launches [`plugin-host`](../../plugin_host/index.html) and offers
//! roughly the in-process registry's surface, with one deliberate difference: calls are
//! dynamically typed. A host binary is compiled before anyone writes a plugin, so it
//! cannot know a `#[lua_class]` trait.
//!
//! ```ignore
//! let mut remote = RemoteRegistry::launch(RemoteOptions::new("./plugin-host"))?;
//! remote.load("plugins/")?;
//! let greeting: String = remote.call("greeter", "greet", [json!("world")])?;
//! ```

mod frame;
mod client;
pub mod protocol;
mod server;

pub use client::{RemoteError, RemoteOptions, RemoteRegistry};
pub use protocol::{
    AuditEntry, CallbackCall, Failure, HostInfo, LoadResult, Outcome, PluginInfo, method,
};
pub use server::{
    build_registry, load_config, serve, CapabilityConfig, HostChannel, HostConfig, SandboxConfig,
    SignatureConfig,
};

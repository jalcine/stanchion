//! Stanchion for languages that are not Rust.
//!
//! This crate is the seam every binding sits on — Python, Kotlin, Swift, Ruby and
//! JavaScript all wrap the same [`Stanchion`] with the same semantics. It carries no
//! binding dependencies itself: `pyo3`, `uniffi`, `magnus` and `neon` each live in
//! their own crate, so none of them can leak a convenience into the shared shape.
//!
//! # What a foreign caller gets, and does not
//!
//! Plugins load through [`stanchion_registry::Registry`], which drives one
//! runtime backend per plugin type (Lua, WASM, …). No foreign language can
//! name a backend's native types, so every boundary here speaks
//! [`stanchion_abi`] interfaces: arguments and results travel as [`Value`],
//! capabilities arrive as [`CapabilityCall`], and policy answers
//! [`Decision`].
//!
//! That is the same trade `plugin-host` makes for the same reason, and deliberately
//! so: the two share a value model and a configuration vocabulary, so an application
//! can move between in-process and out-of-process hosting without a plugin noticing.
//!
//! ```no_run
//! use std::sync::Arc;
//! use stanchion_ffi::{CapabilityCall, CapabilityProvider, Stanchion, Value};
//!
//! struct Log;
//! impl CapabilityProvider for Log {
//!     fn invoke(&self, call: &CapabilityCall) -> Result<Value, String> {
//!         println!("[{}] {:?}", call.plugin, call.args);
//!         Ok(Value::Nil)
//!     }
//! }
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let host = Stanchion::builder().capability("log", Arc::new(Log)).build()?;
//! let report = host.load(Some("plugins/".as_ref()))?;
//! assert!(report.is_clean());
//!
//! let greeting = host.call("greeter", "greet", &[Value::Str("world".into())])?;
//! # Ok(())
//! # }
//! ```
//!
//! # Threading
//!
//! [`Stanchion`] is `Send + Sync` and one lock guards the whole registry, so calls
//! from several threads serialize rather than racing. Two things follow, and both are
//! load-bearing for binding authors:
//!
//! * A capability provider runs while that lock is held. Calling back into the same
//!   [`Stanchion`] from a provider would deadlock, so it returns
//!   [`Error::Reentrant`] instead.
//! * A long-running plugin blocks every other caller for its duration. Instruction
//!   and memory limits are the remedy, and they are on by default.
//!
//! # Features
//!
//! | Feature | Effect |
//! | --- | --- |
//! | `async` | `call_async` and `dispatch_async`; plugin methods may yield |
//! | `signatures` | signer provenance on audits, listings and policy decisions |
//! | `luarocks` | LuaRocks dependency checks |
//! | `lua54`, `lua53`, `luajit`, `luau` | which VM — pick exactly one |
//! | `vendored` | build Lua from source instead of linking the system's |
//!
//! No Lua version is selected by default. A binding that hard-coded one could not be
//! embedded in an application that already has its own VM, so the choice stays with
//! whoever builds the artifact.

mod error;
mod guard;
mod host;
mod value;

pub use error::{Error, Result};
pub use host::{AuditEntry, Builder, Failure, LoadReport, Outcome, PluginInfo, Stanchion};
pub use stanchion_abi::{
    AllowList, CapabilityCall, CapabilityProvider, CapabilityRequest, Decision, Policy,
};
pub use value::Value;

/// The declarative configuration a host is built from.
///
/// Shared with `plugin-host`, which reads the same shape out of a TOML file, so a
/// policy written for one transport describes the other unchanged.
pub use stanchion_lua::config::{
    CapabilityConfig, HostConfig, SandboxConfig, SignatureConfig, load_config,
};

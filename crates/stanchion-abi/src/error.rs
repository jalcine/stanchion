//! One error type, flat enough to survive any backend and any foreign type system.
//!
//! The Rust API distinguishes [`RegistryError`](stanchion_registry::RegistryError),
//! [`LoadFailure`](stanchion_registry::LoadFailure) and runtime errors, each with
//! structured variants worth matching on. Almost none of that structure survives a
//! binding generator or a WASM export: Kotlin sees a sealed class, Ruby sees an
//! exception class, and a deeply nested enum turns into something nobody wants to
//! `match` in any of them.
//!
//! So this flattens to the distinctions a *caller* acts on — is the plugin missing,
//! did its runtime fail, was it refused for provenance — and keeps everything else in
//! the message.

use std::fmt;

/// A runtime error from a foreign environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeError {
    /// The runtime that produced the error, e.g. `"lua"` or `"wasm"`.
    pub runtime_name: String,
    /// The underlying error message.
    pub error: String,
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} runtime error: {}", self.runtime_name, self.error)
    }
}

impl std::error::Error for RuntimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }
}

/// What went wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// No plugin by that name is loaded.
    UnknownPlugin(String),
    /// One plugin failed to load, reload, or verify.
    Plugin { plugin: String, reason: String },
    /// A runtime (Lua, WASM, etc.) raised, or a value could not cross a boundary.
    Runtime(RuntimeError),
    /// A WASM plugin failed to load, run, or answer a call.
    Wasm(String),
    /// The plugin root could not be read.
    Io(String),
    /// The host's own configuration is wrong — an unknown library, a bad root.
    Config(String),
    /// A capability provider refused or failed.
    Capability { capability: String, reason: String },
    /// A capability provider called back into the registry that invoked it.
    ///
    /// The registry is locked for the duration of a call, so this would otherwise
    /// deadlock in silence. See [`crate::PluginBackend`].
    Reentrant,
}

impl Error {
    /// A configuration error from any displayable source.
    pub fn config(err: impl fmt::Display) -> Self {
        Error::Config(err.to_string())
    }

    /// A short, stable tag a binding can map onto its own exception classes.
    ///
    /// Bindings need to name these in several languages without re-deriving the
    /// mapping from the message each time.
    pub fn kind(&self) -> &'static str {
        match self {
            Error::UnknownPlugin(_) => "unknown-plugin",
            Error::Plugin { .. } => "plugin",
            Error::Runtime(_) => "runtime",
            Error::Wasm(_) => "wasm",
            Error::Io(_) => "io",
            Error::Config(_) => "config",
            Error::Capability { .. } => "capability",
            Error::Reentrant => "reentrant",
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::UnknownPlugin(name) => write!(f, "no plugin named `{name}`"),
            Error::Plugin { plugin, reason } => write!(f, "plugin `{plugin}`: {reason}"),
            Error::Runtime(err) => write!(f, "{}", err),
            Error::Wasm(message) => write!(f, "{message}"),
            Error::Io(message) => write!(f, "{message}"),
            Error::Config(message) => write!(f, "configuration: {message}"),
            Error::Capability { capability, reason } => {
                write!(f, "capability `{capability}`: {reason}")
            }
            Error::Reentrant => f.write_str(
                "a capability provider called back into the registry that invoked it, \
                 which would deadlock; do the work without re-entering stanchion",
            ),
        }
    }
}

impl std::error::Error for Error {}

/// The result of anything a foreign caller can ask for.
pub type Result<T> = std::result::Result<T, Error>;

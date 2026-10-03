//! One error type, flat enough to survive any backend and any foreign type system.
//!
//! The Rust API distinguishes `stanchion_registry::RegistryError`,
//! `stanchion_registry::LoadFailure` and runtime errors, each with
//! structured variants worth matching on. (Named rather than linked: this crate is
//! the layer *below* the registry and does not depend on it.) Almost none of that
//! structure survives a
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

impl std::error::Error for RuntimeError {}

/// What went wrong.
///
/// `#[non_exhaustive]`: a new variant is added whenever a new class of failure
/// becomes worth distinguishing, so downstream matches carry a wildcard arm. Without
/// it, `Manifest` landing here broke `stanchion-ffi-c`'s build instead of falling
/// through to its catch-all.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// No plugin by that name is loaded.
    UnknownPlugin(String),
    /// One plugin failed to load, reload, or verify.
    Plugin {
        plugin: String,
        reason: String,
    },
    /// A runtime (Lua, WASM, etc.) raised, or a value could not cross a boundary.
    Runtime(RuntimeError),
    /// A WASM plugin failed to load, run, or answer a call.
    Wasm(String),
    /// The plugin root could not be read.
    Io(String),
    /// The host's own configuration is wrong — an unknown library, a bad root.
    Config(String),
    /// A capability provider refused or failed.
    Capability {
        capability: String,
        reason: String,
    },
    /// A capability provider called back into the host handle that invoked it.
    ///
    /// A plugin call holds its runtime's state lock for the duration, and that lock is
    /// not reentrant, so re-entering would stop the process dead with no error and no
    /// stack.
    ///
    /// **Reported by `stanchion-ffi` only.** Each `Stanchion` handle carries an id and
    /// each thread records which handle it is inside, so the bindings — where a
    /// long-lived host handle makes this an easy mistake — get this error instead of a
    /// hang. `stanchion_registry::Registry` has no equivalent guard: a provider there
    /// cannot borrow the registry that owns it, so reaching the deadlock takes
    /// deliberate effort (routing the registry back to the provider through an `Arc`,
    /// a `OnceLock` or a global). If you do that, you get the hang, not this error.
    ///
    /// This doc used to say flatly that "the registry is locked for the duration of a
    /// call", which read as a guarantee the Rust API does not make.
    Reentrant,
    /// A plugin's `plugin.toml` could not be parsed.
    ///
    /// Carries the path and the parser's message rather than a `toml::de::Error`:
    /// this enum crosses a C ABI and four binding generators, none of which can
    /// render a foreign crate's error type. See `errors_carry_no_foreign_types`.
    Manifest {
        /// The manifest that failed to parse.
        path: String,
        /// What the TOML parser said.
        reason: String,
    },
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
            Error::Manifest { .. } => "manifest",
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
            Error::Manifest { path, reason } => {
                write!(f, "manifest `{path}`: {reason}")
            }
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

impl From<RuntimeError> for Error {
    fn from(err: RuntimeError) -> Self {
        Error::Runtime(err)
    }
}

impl std::error::Error for Error {
    /// Links the one variant that wraps another error, so `anyhow`, `eyre` and
    /// `tracing` can report the cause chain. The rest hold strings: returning them
    /// as their own source would print every message twice.
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Runtime(err) => Some(err),
            _ => None,
        }
    }
}

/// The result of anything a foreign caller can ask for.
pub type Result<T> = std::result::Result<T, Error>;

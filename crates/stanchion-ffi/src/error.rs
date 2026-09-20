//! One error type, flat enough to survive five foreign type systems.
//!
//! The Rust API distinguishes [`RegistryError`], [`LoadFailure`] and [`mlua::Error`],
//! each with structured variants worth matching on. Almost none of that structure
//! survives a binding generator: Kotlin sees a sealed class, Ruby sees an exception
//! class, Python sees a subclass tree, and a deeply nested enum turns into something
//! nobody wants to `match` in any of them.
//!
//! So this flattens to the distinctions a *caller* acts on — is the plugin missing,
//! did its Lua fail, was it refused for provenance — and keeps everything else in the
//! message.

use std::fmt;

use stanchion_registry::{LoadFailure, RegistryError};

/// What went wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// No plugin by that name is loaded.
    UnknownPlugin(String),
    /// One plugin failed to load, reload, or verify.
    Plugin { plugin: String, reason: String },
    /// A plugin's Lua raised, or a value could not cross the boundary.
    Lua(String),
    /// The plugin root could not be read.
    Io(String),
    /// The host's own configuration is wrong — an unknown library, a bad root.
    Config(String),
    /// A capability provider refused or failed.
    Capability { capability: String, reason: String },
    /// A capability provider called back into the registry that invoked it.
    ///
    /// The registry is locked for the duration of a call, so this would otherwise
    /// deadlock in silence. See [`crate::CapabilityProvider`].
    Reentrant,
}

impl Error {
    pub(crate) fn config(err: impl fmt::Display) -> Self {
        Error::Config(err.to_string())
    }

    /// A short, stable tag a binding can map onto its own exception classes.
    ///
    /// Bindings need to name these in five languages without re-deriving the mapping
    /// from the message each time.
    pub fn kind(&self) -> &'static str {
        match self {
            Error::UnknownPlugin(_) => "unknown-plugin",
            Error::Plugin { .. } => "plugin",
            Error::Lua(_) => "lua",
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
            Error::Lua(message) => write!(f, "{message}"),
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

impl From<mlua::Error> for Error {
    fn from(err: mlua::Error) -> Self {
        Error::Lua(err.to_string())
    }
}

impl From<LoadFailure> for Error {
    fn from(failure: LoadFailure) -> Self {
        Error::Plugin {
            plugin: failure.name.clone(),
            reason: failure.reason.to_string(),
        }
    }
}

impl From<RegistryError> for Error {
    fn from(err: RegistryError) -> Self {
        match err {
            RegistryError::UnknownPlugin(name) => Error::UnknownPlugin(name),
            RegistryError::Io { path, source } => {
                Error::Io(format!("reading `{}`: {source}", path.display()))
            }
            RegistryError::Reload(failure) => Error::from(*failure),
            RegistryError::Lua(source) => Error::Lua(source.to_string()),
            // `RegistryError` is `#[non_exhaustive]` in spirit: `Rocks` appears
            // whenever *any* crate in the build turns on the registry's `luarocks`
            // feature, which this crate's own features do not control. Matching the
            // rest by name and falling through keeps that from being a build error.
            #[allow(unreachable_patterns)]
            other => Error::Config(other.to_string()),
        }
    }
}

/// The result of anything a foreign caller can ask for.
pub type Result<T> = std::result::Result<T, Error>;

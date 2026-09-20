//! Errors raised while discovering, loading and reloading plugins.

use std::error::Error;
use std::fmt;
use std::io;
use std::path::PathBuf;

/// A fatal registry error: the caller asked for something that cannot proceed.
///
/// Problems with an individual plugin are reported as a [`LoadFailure`] instead, so one
/// bad plugin never stops the others from loading.
#[derive(Debug)]
pub enum RegistryError {
    /// The plugin root could not be read.
    Io { path: PathBuf, source: io::Error },
    /// A reload named a plugin the registry does not hold.
    UnknownPlugin(String),
    /// A plugin failed while being reloaded, which leaves the old instance in place.
    Reload(Box<LoadFailure>),
    /// A plugin state could not be created or configured.
    Lua(mlua::Error),
    /// The `luarocks` command could not be queried, so no plugin can be verified.
    #[cfg(feature = "luarocks")]
    Rocks(stanchion_rocks::RocksError),
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegistryError::Io { path, source } => {
                write!(f, "reading `{}`: {source}", path.display())
            }
            RegistryError::UnknownPlugin(name) => write!(f, "no plugin named `{name}`"),
            RegistryError::Reload(failure) => write!(f, "reloading {failure}"),
            RegistryError::Lua(source) => write!(f, "creating a plugin state: {source}"),
            #[cfg(feature = "luarocks")]
            RegistryError::Rocks(source) => write!(f, "luarocks: {source}"),
        }
    }
}

impl Error for RegistryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            RegistryError::Io { source, .. } => Some(source),
            RegistryError::UnknownPlugin(_) => None,
            RegistryError::Reload(failure) => Some(&failure.reason),
            RegistryError::Lua(source) => Some(source),
            #[cfg(feature = "luarocks")]
            RegistryError::Rocks(source) => Some(source),
        }
    }
}

/// One plugin that could not be loaded, and why.
#[derive(Debug)]
pub struct LoadFailure {
    /// Plugin name, or the directory name when the manifest itself was unreadable.
    pub name: String,
    /// Directory the plugin was discovered in.
    pub dir: PathBuf,
    /// What went wrong.
    pub reason: FailureReason,
}

impl fmt::Display for LoadFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "plugin `{}` ({}): {}", self.name, self.dir.display(), self.reason)
    }
}

impl Error for LoadFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.reason)
    }
}

/// Why a single plugin failed to load.
#[derive(Debug)]
pub enum FailureReason {
    /// The manifest or entry file could not be read.
    Io(io::Error),
    /// The manifest was not valid TOML, or was missing a required key.
    Manifest(String),
    /// The chunk failed to run, did not return a valid class, or the constructor failed.
    Lua(mlua::Error),
    /// A declared dependency is not present in the plugin root.
    MissingDependency(String),
    /// A dependency is present but its version does not satisfy the requirement.
    IncompatibleDependency {
        /// Dependency name.
        name: String,
        /// The requirement this plugin declared.
        required: String,
        /// The version the dependency actually publishes.
        found: String,
    },
    /// A plugin is wired to a dependency that publishes no `exports` table.
    MissingExports(String),
    /// A dependency lives in another Lua state, so its exports cannot be handed over.
    CrossStateDependency(String),
    /// The plugin carries no signature and the registry requires one.
    Unsigned,
    /// A signature was present but did not verify.
    SignatureInvalid(String),
    /// The signature verified but the signer is not trusted.
    UntrustedSigner(String),
    /// The build or its signer is on the host's revocation list.
    Revoked(String),
    /// A file's bytes changed between verification and loading.
    DigestMismatch(String),
    /// Policy refused a capability the plugin requires.
    CapabilityDenied { name: String, reason: String },
    /// The plugin requested a capability the host does not offer.
    UnknownCapability(String),
    /// A declared rock is not installed in the configured tree.
    MissingRock { name: String, required: String },
    /// A declared rock is installed at a version that fails its requirement.
    IncompatibleRock { name: String, required: String, found: String },
    /// The rocks subsystem could not answer for this plugin.
    Rocks(String),
    /// A declared dependency failed to load, so this plugin was skipped.
    DependencyFailed(String),
    /// This plugin is part of a dependency cycle.
    DependencyCycle(Vec<String>),
}

impl fmt::Display for FailureReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FailureReason::Io(source) => write!(f, "{source}"),
            FailureReason::Manifest(message) => write!(f, "invalid manifest: {message}"),
            FailureReason::Lua(source) => write!(f, "{source}"),
            FailureReason::MissingDependency(name) => {
                write!(f, "depends on `{name}`, which was not found")
            }
            FailureReason::IncompatibleDependency { name, required, found } => {
                write!(f, "requires `{name}` {required}, but it publishes {found}")
            }
            FailureReason::MissingExports(name) => {
                write!(f, "`{name}` declares no `exports` table")
            }
            FailureReason::Unsigned => {
                f.write_str("carries no signature, and this registry requires one")
            }
            FailureReason::SignatureInvalid(message) => write!(f, "{message}"),
            FailureReason::UntrustedSigner(message) => write!(f, "{message}"),
            FailureReason::Revoked(message) => write!(f, "{message}"),
            FailureReason::DigestMismatch(path) => write!(
                f,
                "`{path}` changed between verification and loading"
            ),
            FailureReason::CapabilityDenied { name, reason } => {
                write!(f, "capability `{name}` denied: {reason}")
            }
            FailureReason::UnknownCapability(name) => {
                write!(f, "requests capability `{name}`, which the host does not offer")
            }
            FailureReason::CrossStateDependency(name) => write!(
                f,
                "depends on `{name}`, but per-plugin isolation gives each plugin its own \
                 Lua state and values cannot cross states"
            ),
            FailureReason::MissingRock { name, required } => {
                write!(f, "rock `{name}` {required} is not installed")
            }
            FailureReason::IncompatibleRock { name, required, found } => {
                write!(f, "rock `{name}` {required} is installed at {found}")
            }
            FailureReason::Rocks(message) => write!(f, "{message}"),
            FailureReason::DependencyFailed(name) => {
                write!(f, "skipped because `{name}` failed to load")
            }
            FailureReason::DependencyCycle(names) => {
                write!(f, "dependency cycle: {}", names.join(" -> "))
            }
        }
    }
}

impl Error for FailureReason {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            FailureReason::Io(source) => Some(source),
            FailureReason::Lua(source) => Some(source),
            _ => None,
        }
    }
}

impl From<mlua::Error> for FailureReason {
    fn from(source: mlua::Error) -> Self {
        FailureReason::Lua(source)
    }
}

impl From<io::Error> for FailureReason {
    fn from(source: io::Error) -> Self {
        FailureReason::Io(source)
    }
}

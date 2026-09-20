//! Pinned plugin builds: the trust anchor for [distribution](../../../docs/distribution.md).
//!
//! A [`Lockfile`] states, for each plugin the host expects, the exact root digest that
//! may load and — when the host cares — who must have signed it. That statement is
//! what makes a transport untrustworthy by design: a registry, mirror or index can
//! serve whatever it likes, and anything other than the pinned bytes fails to load.
//!
//! # Why pin rather than resolve
//!
//! `[dependencies] formatter = "^1.0"` says which versions are *acceptable*. It cannot
//! say which `formatter` is the real one, and a range is exactly the room an attacker
//! needs: substitute a different build, or downgrade to an older signed-but-vulnerable
//! one, and every signature still verifies. Neither attack requires forging anything.
//!
//! Pinning moves that decision out of process start and into an explicit, reviewable
//! update — where a human can read a [capability diff](crate::upgrade) before the new
//! digest is written down.
//!
//! # What a lockfile does not buy
//!
//! - **It pins bytes, not behaviour.** The pinned build is the one the host reviewed;
//!   whether that build is safe is what the review was for.
//! - **It says nothing about how the entry got there.** A lockfile written from an
//!   index that lied pins the lie. The signer pin and the capability diff are what
//!   make writing an entry a decision rather than a transcription.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::Path;

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::manifest::Manifest;
use crate::signature::{DirectoryDigest, Signer};

/// Name of the lockfile a host conventionally keeps beside its plugin root.
pub const LOCK_FILE: &str = "stanchion.lock";

/// Lockfile format version this crate writes and accepts.
pub const LOCK_VERSION: u32 = 1;

/// Digest prefix, spelled out so a lock entry says which hash it is.
const SHA256_PREFIX: &str = "sha256:";

/// Why a plugin did not match the lockfile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockError {
    /// The lockfile has no entry for this plugin.
    Unlocked(String),
    /// The plugin's bytes are not the pinned bytes.
    DigestMismatch {
        /// Plugin name.
        name: String,
        /// The digest the lockfile pins.
        expected: String,
        /// The digest the directory actually produced.
        found: String,
    },
    /// The plugin is signed, but not by the pinned identity.
    SignerMismatch {
        /// Plugin name.
        name: String,
        /// The identity the lockfile pins.
        expected: String,
        /// Who actually signed, or `unsigned`.
        found: String,
    },
    /// The manifest's version is not the pinned version.
    VersionMismatch {
        /// Plugin name.
        name: String,
        /// The version the lockfile pins.
        expected: Version,
        /// The version the manifest declares.
        found: Version,
    },
    /// The lockfile itself could not be read or understood.
    Malformed(String),
}

impl fmt::Display for LockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LockError::Unlocked(name) => {
                write!(f, "`{name}` is not in the lockfile")
            }
            LockError::DigestMismatch { name, expected, found } => write!(
                f,
                "`{name}` is pinned to {expected} but these bytes are {found}"
            ),
            LockError::SignerMismatch { name, expected, found } => write!(
                f,
                "`{name}` is pinned to signer `{expected}` but was signed by {found}"
            ),
            LockError::VersionMismatch { name, expected, found } => {
                write!(f, "`{name}` is pinned at {expected} but declares {found}")
            }
            LockError::Malformed(message) => write!(f, "{message}"),
        }
    }
}

impl Error for LockError {}

/// One pinned plugin build.
///
/// `digest` is the only field that has to be there. The rest narrow what is accepted,
/// and `source` is a hint for the fetcher that carries no authority at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedPlugin {
    /// Root digest of the plugin directory, as `sha256:<hex>`.
    pub digest: String,
    /// Version the manifest must declare, when the host wants that pinned too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<Version>,
    /// Identity that must have signed this build.
    ///
    /// Pinning this is what separates "verified by someone the trust root accepts"
    /// from "verified by the author I chose", which is a much stronger claim than it
    /// is usually read as.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signer: Option<String>,
    /// Issuer that must have vouched for the signer, where the scheme records one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer: Option<String>,
    /// Where this build was fetched from, for tooling to fetch it again.
    ///
    /// A hint, never an authority: the digest decides what may load, so changing this
    /// changes where bytes come from and nothing about whether they are accepted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl LockedPlugin {
    /// Pins a build by digest alone.
    pub fn new(digest: impl Into<String>) -> Self {
        LockedPlugin {
            digest: normalize_digest(&digest.into()),
            version: None,
            signer: None,
            issuer: None,
            source: None,
        }
    }

    /// Pins the build a directory currently holds.
    pub fn from_digest(digest: &DirectoryDigest) -> Self {
        LockedPlugin::new(format!("{SHA256_PREFIX}{}", digest.hex()))
    }

    /// Also requires this signer identity.
    pub fn signed_by(mut self, identity: impl Into<String>) -> Self {
        self.signer = Some(identity.into());
        self
    }

    /// Also requires this issuer.
    pub fn issued_by(mut self, issuer: impl Into<String>) -> Self {
        self.issuer = Some(issuer.into());
        self
    }

    /// Also requires this manifest version.
    pub fn at_version(mut self, version: Version) -> Self {
        self.version = Some(version);
        self
    }

    /// Records where the build came from.
    pub fn fetched_from(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// The pinned digest as bare lowercase hex, without the `sha256:` prefix.
    pub fn hex(&self) -> &str {
        self.digest.strip_prefix(SHA256_PREFIX).unwrap_or(&self.digest)
    }
}

/// Every plugin build a host accepts, by name.
///
/// ```toml
/// version = 1
///
/// [plugins.formatter]
/// version = "1.4.2"
/// digest = "sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
/// signer = "repo:acme/plugins"
/// source = "oci://ghcr.io/acme/plugins/formatter:1.4.2"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lockfile {
    /// Format version, so an older host refuses a newer file instead of misreading it.
    #[serde(default = "default_lock_version")]
    pub version: u32,
    /// Pinned builds, by plugin name.
    #[serde(default)]
    pub plugins: BTreeMap<String, LockedPlugin>,
}

fn default_lock_version() -> u32 {
    LOCK_VERSION
}

impl Default for Lockfile {
    fn default() -> Self {
        Lockfile { version: LOCK_VERSION, plugins: BTreeMap::new() }
    }
}

impl Lockfile {
    /// An empty lockfile, which pins nothing.
    pub fn new() -> Self {
        Lockfile::default()
    }

    /// Reads a lockfile from disk.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, LockError> {
        let path = path.as_ref();
        let source = fs::read_to_string(path)
            .map_err(|err| LockError::Malformed(format!("{}: {err}", path.display())))?;
        Lockfile::parse(&source)
            .map_err(|err| LockError::Malformed(format!("{}: {err}", path.display())))
    }

    /// Reads a lockfile from disk, treating an absent file as pinning nothing.
    ///
    /// Useful for the first run of a host that writes its lockfile as it installs.
    pub fn load_or_empty(path: impl AsRef<Path>) -> Result<Self, LockError> {
        match fs::read_to_string(path.as_ref()) {
            Ok(source) => Lockfile::parse(&source).map_err(|err| {
                LockError::Malformed(format!("{}: {err}", path.as_ref().display()))
            }),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Lockfile::new()),
            Err(err) => Err(LockError::Malformed(format!(
                "{}: {err}",
                path.as_ref().display()
            ))),
        }
    }

    /// Parses lockfile TOML.
    pub fn parse(source: &str) -> Result<Self, LockError> {
        let mut lock: Lockfile =
            toml::from_str(source).map_err(|err| LockError::Malformed(err.to_string()))?;

        if lock.version > LOCK_VERSION {
            return Err(LockError::Malformed(format!(
                "lockfile format version {} is newer than this build understands ({LOCK_VERSION})",
                lock.version
            )));
        }

        for (name, entry) in &mut lock.plugins {
            entry.digest = normalize_digest(&entry.digest);
            let hex = entry.hex();
            if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(LockError::Malformed(format!(
                    "`{name}`: `{}` is not a sha256 digest",
                    entry.digest
                )));
            }
        }
        Ok(lock)
    }

    /// Renders the lockfile as TOML.
    pub fn to_toml(&self) -> Result<String, LockError> {
        toml::to_string_pretty(self).map_err(|err| LockError::Malformed(err.to_string()))
    }

    /// Writes the lockfile to disk.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), LockError> {
        let rendered = self.to_toml()?;
        fs::write(path.as_ref(), rendered).map_err(|err| {
            LockError::Malformed(format!("{}: {err}", path.as_ref().display()))
        })
    }

    /// Pins one plugin, replacing any existing entry.
    pub fn pin(&mut self, name: impl Into<String>, entry: LockedPlugin) -> Option<LockedPlugin> {
        self.plugins.insert(name.into(), entry)
    }

    /// The entry for one plugin, if it is pinned.
    pub fn get(&self, name: &str) -> Option<&LockedPlugin> {
        self.plugins.get(name)
    }

    /// Whether this lockfile pins nothing.
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Checks a plugin's bytes and signer against its pin.
    ///
    /// A plugin with no entry is [`LockError::Unlocked`]: once a host keeps a
    /// lockfile, an unpinned plugin appearing in the root is the thing worth
    /// noticing, so the caller decides whether that is fatal rather than this
    /// silently allowing it.
    pub fn check(
        &self,
        manifest: &Manifest,
        digest: &DirectoryDigest,
        signer: &Signer,
    ) -> Result<(), LockError> {
        let name = &manifest.name;
        let Some(entry) = self.plugins.get(name) else {
            return Err(LockError::Unlocked(name.clone()));
        };

        let found = digest.hex();
        if !entry.hex().eq_ignore_ascii_case(&found) {
            return Err(LockError::DigestMismatch {
                name: name.clone(),
                expected: entry.digest.clone(),
                found: format!("{SHA256_PREFIX}{found}"),
            });
        }

        if let Some(expected) = &entry.signer
            && signer.identity() != Some(expected.as_str())
        {
            return Err(LockError::SignerMismatch {
                name: name.clone(),
                expected: expected.clone(),
                found: signer.to_string(),
            });
        }

        if let Some(expected) = &entry.issuer
            && signer.issuer() != Some(expected.as_str())
        {
            return Err(LockError::SignerMismatch {
                name: name.clone(),
                expected: expected.clone(),
                found: signer.issuer().unwrap_or("no issuer").to_string(),
            });
        }

        if let Some(expected) = &entry.version {
            let found = manifest.effective_version();
            if *expected != found {
                return Err(LockError::VersionMismatch {
                    name: name.clone(),
                    expected: expected.clone(),
                    found,
                });
            }
        }

        Ok(())
    }
}

/// Accepts a bare hex digest as well as the `sha256:` form, and lowercases both.
fn normalize_digest(digest: &str) -> String {
    let trimmed = digest.trim();
    match trimmed.strip_prefix(SHA256_PREFIX) {
        Some(hex) => format!("{SHA256_PREFIX}{}", hex.to_ascii_lowercase()),
        None => format!("{SHA256_PREFIX}{}", trimmed.to_ascii_lowercase()),
    }
}

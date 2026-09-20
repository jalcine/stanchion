//! Plugin signatures: binding an identity to the exact bytes that will run.
//!
//! # What is signed
//!
//! Every file in the plugin directory, not just its manifest. Signing `plugin.toml`
//! alone would be worse than useless: an attacker could swap `init.lua`, leave the
//! manifest untouched, and an audit would report capabilities the code does not match.
//!
//! [`DirectoryDigest`] hashes each file and folds them, in sorted order, into one root
//! digest. That root is what a [`PluginVerifier`] checks. The per-file hashes are kept
//! so the loader can confirm, at the moment it reads a file, that those are the bytes
//! that were verified — closing the window between checking and loading.
//!
//! # What a signature does not buy
//!
//! Origin and integrity, not safety: a verified plugin from a trusted author can still
//! be buggy or hostile. A signature tells you whom to hold responsible, not whether to
//! worry. Revocation is a separate problem — a withdrawn plugin's signature stays valid.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

/// Detached signature file an ed25519-style verifier looks for.
pub const SIGNATURE_FILE: &str = "plugin.sig";

/// Sigstore bundle file, produced by `cosign sign-blob --bundle`.
pub const BUNDLE_FILE: &str = "plugin.sigstore.json";

/// Files never covered by the digest, because they carry the signature itself.
pub const EXCLUDED: &[&str] = &[SIGNATURE_FILE, BUNDLE_FILE];

/// Who signed a plugin, as far as the host could establish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signer {
    /// No signature was present, and the registry was configured to allow that.
    Unsigned,
    /// A signature verified against the host's trust root.
    Verified {
        /// Subject identity — a key name, an email, or a workload identity such as
        /// `repo:acme/plugins`.
        identity: String,
        /// Issuer that vouched for the identity, where the scheme has one.
        issuer: Option<String>,
    },
}

impl Signer {
    /// Convenience constructor for a verified signer.
    pub fn verified(identity: impl Into<String>, issuer: Option<String>) -> Self {
        Signer::Verified { identity: identity.into(), issuer }
    }

    /// The verified identity, or `None` when unsigned.
    pub fn identity(&self) -> Option<&str> {
        match self {
            Signer::Unsigned => None,
            Signer::Verified { identity, .. } => Some(identity),
        }
    }

    /// The issuer, where the scheme records one.
    pub fn issuer(&self) -> Option<&str> {
        match self {
            Signer::Unsigned => None,
            Signer::Verified { issuer, .. } => issuer.as_deref(),
        }
    }

    /// Whether a signature was verified at all.
    pub fn is_verified(&self) -> bool {
        matches!(self, Signer::Verified { .. })
    }
}

impl fmt::Display for Signer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Signer::Unsigned => f.write_str("unsigned"),
            Signer::Verified { identity, issuer: Some(issuer) } => {
                write!(f, "{identity} (via {issuer})")
            }
            Signer::Verified { identity, issuer: None } => f.write_str(identity),
        }
    }
}

/// Why verification did not produce a trusted signer.
#[derive(Debug)]
pub enum VerifyError {
    /// No signature artifact was found in the plugin directory.
    Missing,
    /// A signature was present but did not check out against the digest.
    Invalid(String),
    /// The signature is cryptographically sound but the signer is not trusted.
    Untrusted(String),
    /// The signature artifact could not be read.
    Io(io::Error),
}

impl fmt::Display for VerifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VerifyError::Missing => f.write_str("no signature found"),
            VerifyError::Invalid(message) => write!(f, "signature is invalid: {message}"),
            VerifyError::Untrusted(message) => write!(f, "signer is not trusted: {message}"),
            VerifyError::Io(source) => write!(f, "reading the signature: {source}"),
        }
    }
}

impl Error for VerifyError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            VerifyError::Io(source) => Some(source),
            _ => None,
        }
    }
}

impl From<io::Error> for VerifyError {
    fn from(source: io::Error) -> Self {
        VerifyError::Io(source)
    }
}

/// A canonical digest over every file in a plugin directory.
///
/// The root is `sha256` over, for each file in sorted order:
/// `relative path ‖ 0x00 ‖ sha256(contents) ‖ 0x00`. Paths use `/` separators so the
/// digest is stable across platforms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryDigest {
    root: [u8; 32],
    preimage: Vec<u8>,
    files: BTreeMap<String, [u8; 32]>,
}

impl DirectoryDigest {
    /// Hashes every file under `dir`, skipping [`EXCLUDED`].
    pub fn compute(dir: &Path) -> io::Result<Self> {
        let mut entries = Vec::new();
        collect_files(dir, dir, &mut entries)?;

        let mut files: BTreeMap<String, [u8; 32]> = BTreeMap::new();
        for (relative, path) in entries {
            if EXCLUDED.contains(&relative.as_str()) {
                continue;
            }
            let contents = fs::read(&path)?;
            files.insert(relative, Sha256::digest(&contents).into());
        }

        // BTreeMap iteration is sorted, which is what makes the root reproducible.
        let mut preimage = Vec::new();
        for (relative, hash) in &files {
            preimage.extend_from_slice(relative.as_bytes());
            preimage.push(0);
            preimage.extend_from_slice(hash);
            preimage.push(0);
        }

        let root: [u8; 32] = Sha256::digest(&preimage).into();
        Ok(DirectoryDigest { root, preimage, files })
    }

    /// The root digest, which is what gets signed.
    pub fn root(&self) -> &[u8; 32] {
        &self.root
    }

    /// The exact bytes the root digest is taken over.
    ///
    /// This is the artifact a signature covers, so external tooling can sign the same
    /// bytes without reimplementing the layout:
    ///
    /// ```text
    /// for each file, sorted by path:
    ///     relative path ‖ 0x00 ‖ sha256(contents) ‖ 0x00
    /// ```
    ///
    /// Write it to a file and `cosign sign-blob --bundle plugin.sigstore.json` it.
    pub fn preimage(&self) -> &[u8] {
        &self.preimage
    }

    /// The root digest as lowercase hex.
    pub fn hex(&self) -> String {
        to_hex(&self.root)
    }

    /// Number of files covered.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Whether the directory held no coverable files.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Confirms that `contents` are the bytes recorded for `relative` at digest time.
    ///
    /// The loader calls this as it reads each file, so what actually runs is what was
    /// verified, even if the directory changed in between.
    pub fn matches(&self, relative: &str, contents: &[u8]) -> bool {
        let actual: [u8; 32] = Sha256::digest(contents).into();
        self.files.get(relative).is_some_and(|expected| *expected == actual)
    }

    /// Whether the digest covers this relative path at all.
    pub fn covers(&self, relative: &str) -> bool {
        self.files.contains_key(relative)
    }
}

/// Checks a plugin's signature against the host's trust root.
///
/// Implementations decide what counts as trusted; the registry only decides what to do
/// with the answer.
pub trait PluginVerifier: mlua::MaybeSend + mlua::MaybeSync {
    /// Verifies `dir`'s signature over `digest`, returning who signed it.
    ///
    /// Return [`VerifyError::Missing`] when no signature artifact is present, so the
    /// registry can apply its unsigned-plugin policy rather than treating it as fraud.
    fn verify(&self, digest: &DirectoryDigest, dir: &Path) -> Result<Signer, VerifyError>;
}

/// Walks `dir` recursively, recording `/`-separated paths relative to `root`.
fn collect_files(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect_files(root, &path, out)?;
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| io::Error::other("path escaped the plugin directory"))?
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        out.push((relative, path));
    }
    Ok(())
}

/// Lowercase hex, without pulling in a dependency for it.
pub(crate) fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        out.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        out.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0'));
    }
    out
}



/// One entry on a revocation list.
///
/// Either a plugin build (by digest) or a signer (by identity), with an optional
/// reason the host can log or show.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Revocation {
    /// Hex root digest of a specific plugin build.
    #[serde(default)]
    pub digest: Option<String>,
    /// Signer identity, revoking everything that identity signed.
    #[serde(default)]
    pub identity: Option<String>,
    /// Why, for the host's logs.
    #[serde(default)]
    pub reason: Option<String>,
}

impl Revocation {
    fn describe(&self, what: &str) -> String {
        match &self.reason {
            Some(reason) => format!("{what} is revoked: {reason}"),
            None => format!("{what} is revoked"),
        }
    }
}

/// Builds and plugins the host refuses to load, whatever their signature says.
///
/// A signature proves who produced a plugin; it cannot say the plugin was later
/// withdrawn. Revocation is the separate, mutable half of that story, so it is checked
/// *after* verification succeeds — a revoked signature is still a valid signature.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Revocations {
    #[serde(default)]
    revoked: Vec<Revocation>,
}

impl Revocations {
    /// An empty list, which refuses nothing.
    pub fn new() -> Self {
        Revocations::default()
    }

    /// Refuses one plugin build by its hex root digest.
    pub fn deny_digest(mut self, digest: impl Into<String>, reason: impl Into<String>) -> Self {
        self.revoked.push(Revocation {
            digest: Some(digest.into()),
            identity: None,
            reason: Some(reason.into()),
        });
        self
    }

    /// Refuses everything signed by one identity.
    pub fn deny_identity(mut self, identity: impl Into<String>, reason: impl Into<String>) -> Self {
        self.revoked.push(Revocation {
            digest: None,
            identity: Some(identity.into()),
            reason: Some(reason.into()),
        });
        self
    }

    /// Reads a TOML revocation list.
    ///
    /// ```toml
    /// [[revoked]]
    /// digest = "9f86d081884c7d65…"
    /// reason = "CVE-2026-1234"
    ///
    /// [[revoked]]
    /// identity = "repo:acme/compromised"
    /// reason = "key compromise"
    /// ```
    ///
    /// An entry naming neither a digest nor an identity is a configuration error
    /// rather than an entry that silently matches nothing.
    pub fn load(path: &Path) -> Result<Self, VerifyError> {
        let source = fs::read_to_string(path)?;
        let list: Revocations = toml::from_str(&source)
            .map_err(|err| VerifyError::Invalid(format!("{}: {err}", path.display())))?;

        for (position, entry) in list.revoked.iter().enumerate() {
            if entry.digest.is_none() && entry.identity.is_none() {
                return Err(VerifyError::Invalid(format!(
                    "{}: revoked entry {} names neither `digest` nor `identity`",
                    path.display(),
                    position.saturating_add(1)
                )));
            }
        }
        Ok(list)
    }

    /// The entries on the list.
    pub fn entries(&self) -> &[Revocation] {
        &self.revoked
    }

    /// Whether the list refuses nothing.
    pub fn is_empty(&self) -> bool {
        self.revoked.is_empty()
    }

    /// Returns why this plugin is refused, or `None` if it is not.
    ///
    /// Digests are compared case-insensitively so a list written by hand still matches.
    pub fn check(&self, digest: Option<&DirectoryDigest>, signer: &Signer) -> Option<String> {
        let hex = digest.map(DirectoryDigest::hex);

        for entry in &self.revoked {
            if let (Some(revoked), Some(actual)) = (&entry.digest, &hex)
                && revoked.trim().eq_ignore_ascii_case(actual)
            {
                return Some(entry.describe("this build"));
            }
            if let (Some(revoked), Some(actual)) = (&entry.identity, signer.identity())
                && revoked == actual
            {
                return Some(entry.describe(&format!("signer `{actual}`")));
            }
        }
        None
    }
}

//! The index protocol: how a host learns that a plugin exists and what to pin.
//!
//! An index is **two static JSON documents**. Anything that can serve a file can run
//! one — S3, GitHub Pages, a directory behind nginx, a git repository someone clones.
//! There is no database, no API and no server to operate, because an index holds no
//! authority worth protecting.
//!
//! ```text
//! <base>/v1/index.json              the catalog: which plugins exist
//! <base>/v1/plugins/<name>.json     the releases of one plugin
//! ```
//!
//! # The index is not trusted
//!
//! This is the whole design, and it is worth being blunt about. An index answers one
//! question — *"which digests claim to be `formatter` 1.4.x?"* — and nothing it says
//! is taken on faith:
//!
//! - **It cannot substitute bytes.** What an index returns is a digest, which the
//!   installer checks against the bytes it actually fetched and the host then pins in
//!   its [lockfile](stanchion_registry::lock). A lying index produces a mismatch.
//! - **It cannot widen authority.** The `capabilities` field here is advisory, for
//!   browsing. Authority comes from the manifest inside the package, and an upgrade
//!   that asks for more is caught by an [upgrade review](stanchion_registry::upgrade)
//!   before it is written down.
//! - **It cannot silently downgrade or freeze you.** Every document carries an
//!   [`expires`](IndexDocument::expires); a client refuses a stale one rather than
//!   trusting a snapshot an attacker withheld an update from.
//!
//! What an index *can* still do is refuse to tell you about a release, or offer you a
//! genuine-but-old one. Expiry bounds how long either lasts; nothing makes a source
//! you cannot reach tell you the truth.
//!
//! # Running one
//!
//! Generate the documents, publish them, and re-publish before they expire. The
//! re-publishing is the point: an index that is not maintained stops being believed,
//! which is the behaviour you want from a stale mirror.
//!
//! # Implementing one
//!
//! Serve those two paths, or implement [`PluginIndex`] against whatever you already
//! have — an internal artifact service, a database, a directory on a build machine.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// Schema version of the index documents this crate writes and accepts.
pub const INDEX_SCHEMA: u32 = 1;

/// Path of the catalog document, relative to an index base.
pub const CATALOG_PATH: &str = "v1/index.json";

/// Path of one plugin's release document, relative to an index base.
pub fn releases_path(name: &str) -> String {
    format!("v1/plugins/{name}.json")
}

/// Why an index could not answer.
#[derive(Debug)]
pub enum IndexError {
    /// The index has no document for this plugin.
    UnknownPlugin(String),
    /// No release satisfies the requirement.
    NoMatch {
        /// Plugin name.
        name: String,
        /// The requirement that matched nothing.
        requirement: String,
    },
    /// The document could not be read or parsed.
    Malformed(String),
    /// The document is past its expiry, so it may be a withheld snapshot.
    Stale {
        /// Which document.
        what: String,
        /// When it expired.
        expired: String,
    },
    /// The index could not be reached.
    Transport(String),
}

impl fmt::Display for IndexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IndexError::UnknownPlugin(name) => write!(f, "the index has no plugin `{name}`"),
            IndexError::NoMatch { name, requirement } => {
                write!(f, "no release of `{name}` satisfies {requirement}")
            }
            IndexError::Malformed(message) => write!(f, "malformed index: {message}"),
            IndexError::Stale { what, expired } => write!(
                f,
                "{what} expired at {expired}; refusing it rather than trusting a \
                 snapshot that may have been withheld"
            ),
            IndexError::Transport(message) => write!(f, "reaching the index: {message}"),
        }
    }
}

impl Error for IndexError {}

/// Fields every index document carries.
pub trait IndexDocument {
    /// Schema version the document declares.
    fn schema(&self) -> u32;
    /// When the document was generated, RFC 3339.
    fn updated(&self) -> Option<&str>;
    /// When the document stops being believable, RFC 3339.
    ///
    /// An index without one can be frozen indefinitely by anyone who can stop you
    /// reaching the real thing, so [`Freshness::Required`] refuses it.
    fn expires(&self) -> Option<&str>;
}

/// How strictly a client treats a document's expiry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Freshness {
    /// Every document must carry an unexpired `expires`.
    ///
    /// The right setting for an index reached over a network you do not control.
    #[default]
    Required,
    /// Honour `expires` when present, accept a document without one.
    ///
    /// Reasonable for an index you generate and ship yourself, where there is no
    /// separate party who could withhold an update.
    Lenient,
    /// Ignore expiry entirely.
    ///
    /// For offline use and tests. A frozen index is indistinguishable from a current
    /// one under this setting.
    Ignored,
}

impl Freshness {
    /// Checks a document against the current time.
    pub fn check(self, document: &impl IndexDocument, what: &str) -> Result<(), IndexError> {
        self.check_at(document, what, OffsetDateTime::now_utc())
    }

    /// Checks a document against a caller-supplied clock.
    ///
    /// Separate from [`Freshness::check`] so an expiry policy can be tested without
    /// waiting for one, and so a host with its own trusted time source can use it.
    pub fn check_at(
        self,
        document: &impl IndexDocument,
        what: &str,
        now: OffsetDateTime,
    ) -> Result<(), IndexError> {
        if self == Freshness::Ignored {
            return Ok(());
        }
        let Some(expires) = document.expires() else {
            return match self {
                Freshness::Required => Err(IndexError::Malformed(format!(
                    "{what} carries no `expires`, and this client requires one"
                ))),
                _ => Ok(()),
            };
        };

        let deadline = OffsetDateTime::parse(expires, &Rfc3339).map_err(|err| {
            IndexError::Malformed(format!("{what}: `{expires}` is not an RFC 3339 timestamp: {err}"))
        })?;
        if now > deadline {
            return Err(IndexError::Stale {
                what: what.to_string(),
                expired: expires.to_string(),
            });
        }
        Ok(())
    }
}

/// The catalog: which plugins this index knows about.
///
/// ```json
/// {
///   "schema": 1,
///   "name": "acme plugins",
///   "updated": "2026-09-20T00:00:00Z",
///   "expires": "2026-09-27T00:00:00Z",
///   "plugins": ["formatter", "weather"]
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Catalog {
    /// Schema version.
    #[serde(default = "default_schema")]
    pub schema: u32,
    /// Human-readable name of this index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// When this document was generated, RFC 3339.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated: Option<String>,
    /// When this document stops being believable, RFC 3339.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<String>,
    /// Plugin names, each of which has a release document.
    #[serde(default)]
    pub plugins: Vec<String>,
}

fn default_schema() -> u32 {
    INDEX_SCHEMA
}

impl IndexDocument for Catalog {
    fn schema(&self) -> u32 {
        self.schema
    }
    fn updated(&self) -> Option<&str> {
        self.updated.as_deref()
    }
    fn expires(&self) -> Option<&str> {
        self.expires.as_deref()
    }
}

/// Every published release of one plugin.
///
/// ```json
/// {
///   "schema": 1,
///   "name": "formatter",
///   "updated": "2026-09-20T00:00:00Z",
///   "expires": "2026-09-27T00:00:00Z",
///   "releases": [
///     {
///       "version": "1.4.2",
///       "digest": "sha256:9f86d081884c7d65…",
///       "source": "https://plugins.acme.test/v1/blobs/sha256:9f86d081884c7d65…",
///       "signer": "repo:acme/plugins",
///       "capabilities": ["network"]
///     }
///   ]
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginReleases {
    /// Schema version.
    #[serde(default = "default_schema")]
    pub schema: u32,
    /// Plugin name, which must match the document's path.
    pub name: String,
    /// When this document was generated, RFC 3339.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated: Option<String>,
    /// When this document stops being believable, RFC 3339.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<String>,
    /// Published releases, in any order.
    #[serde(default)]
    pub releases: Vec<Release>,
}

impl IndexDocument for PluginReleases {
    fn schema(&self) -> u32 {
        self.schema
    }
    fn updated(&self) -> Option<&str> {
        self.updated.as_deref()
    }
    fn expires(&self) -> Option<&str> {
        self.expires.as_deref()
    }
}

impl PluginReleases {
    /// The newest release satisfying `requirement`, skipping yanked ones.
    ///
    /// Ordering is by semver, not by the order the document lists them, so an index
    /// cannot steer a client by reordering its own file.
    pub fn best_match(&self, requirement: &VersionReq) -> Option<&Release> {
        self.releases
            .iter()
            .filter(|release| !release.yanked && requirement.matches(&release.version))
            .max_by(|left, right| left.version.cmp(&right.version))
    }

    /// One exact version, yanked or not.
    pub fn exact(&self, version: &Version) -> Option<&Release> {
        self.releases
            .iter()
            .find(|release| release.version == *version)
    }

    /// Checks the document is self-consistent and one this client understands.
    pub fn validate(&self, expected_name: &str) -> Result<(), IndexError> {
        if self.schema > INDEX_SCHEMA {
            return Err(IndexError::Malformed(format!(
                "`{}` declares schema {}, newer than this client understands ({INDEX_SCHEMA})",
                self.name, self.schema
            )));
        }
        if self.name != expected_name {
            return Err(IndexError::Malformed(format!(
                "document for `{expected_name}` declares the name `{}`",
                self.name
            )));
        }
        let mut seen = BTreeMap::new();
        for release in &self.releases {
            if seen.insert(release.version.clone(), ()).is_some() {
                return Err(IndexError::Malformed(format!(
                    "`{}` lists version {} more than once",
                    self.name, release.version
                )));
            }
            release.validate(&self.name)?;
        }
        Ok(())
    }
}

/// One published build.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Release {
    /// Semver version, which must match the manifest inside the package.
    pub version: Version,
    /// Root [`DirectoryDigest`](stanchion_registry::DirectoryDigest) as `sha256:<hex>`.
    ///
    /// This is the value that gets pinned, and the only field here with any weight:
    /// everything else the index says is checked against the package once it arrives.
    pub digest: String,
    /// Where to fetch the package, as a URL.
    ///
    /// A hint. Fetching from anywhere else and getting the same digest is equally
    /// acceptable, which is what lets a host mirror without re-signing anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Identity expected to have signed this build.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signer: Option<String>,
    /// Issuer expected to have vouched for the signer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer: Option<String>,
    /// Capabilities the manifest declares, for browsing.
    ///
    /// Advisory only. The manifest inside the package is what the registry reads, and
    /// a disagreement between the two is the index being wrong, not the plugin.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    /// Whether this release has been withdrawn.
    ///
    /// A yank keeps existing pins working and stops new ones being made. It is not
    /// [revocation](stanchion_registry::Revocations): a yanked release that is already
    /// pinned still loads, which is why a host that needs a build to *stop running*
    /// needs a revocation entry as well.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub yanked: bool,
    /// Why it was yanked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yanked_reason: Option<String>,
}

impl Release {
    /// A release pinning one digest.
    pub fn new(version: Version, digest: impl Into<String>) -> Self {
        Release {
            version,
            digest: digest.into(),
            source: None,
            signer: None,
            issuer: None,
            capabilities: Vec::new(),
            yanked: false,
            yanked_reason: None,
        }
    }

    /// Records where the package can be fetched.
    pub fn from_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// Records who signed it.
    pub fn signed_by(mut self, identity: impl Into<String>) -> Self {
        self.signer = Some(identity.into());
        self
    }

    /// The digest as bare lowercase hex.
    pub fn hex(&self) -> &str {
        self.digest.strip_prefix("sha256:").unwrap_or(&self.digest)
    }

    /// Checks the digest is a well-formed sha256.
    fn validate(&self, plugin: &str) -> Result<(), IndexError> {
        let hex = self.hex();
        if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(IndexError::Malformed(format!(
                "`{plugin}` {}: `{}` is not a sha256 digest",
                self.version, self.digest
            )));
        }
        Ok(())
    }

    /// The pin this release would produce.
    pub fn to_pin(&self) -> stanchion_registry::LockedPlugin {
        let mut pin = stanchion_registry::LockedPlugin::new(self.digest.clone())
            .at_version(self.version.clone());
        if let Some(signer) = &self.signer {
            pin = pin.signed_by(signer.clone());
        }
        if let Some(issuer) = &self.issuer {
            pin = pin.issued_by(issuer.clone());
        }
        if let Some(source) = &self.source {
            pin = pin.fetched_from(source.clone());
        }
        pin
    }
}

/// Somewhere a host can ask which releases of a plugin exist.
///
/// Implement this against whatever you already run. The contract is deliberately thin
/// because an index is not trusted: it maps a name to candidate digests, and every
/// claim it makes is checked against the package that actually arrives.
pub trait PluginIndex {
    /// Every published release of one plugin.
    fn releases(&self, name: &str) -> Result<PluginReleases, IndexError>;

    /// Which plugins this index knows about.
    ///
    /// Optional: an index that cannot enumerate returns [`IndexError::UnknownPlugin`]
    /// and stays usable for everything else.
    fn catalog(&self) -> Result<Catalog, IndexError> {
        Err(IndexError::UnknownPlugin("<catalog>".to_string()))
    }

    /// Resolves a requirement to one release.
    fn resolve(&self, name: &str, requirement: &VersionReq) -> Result<Release, IndexError> {
        let releases = self.releases(name)?;
        releases
            .best_match(requirement)
            .cloned()
            .ok_or_else(|| IndexError::NoMatch {
                name: name.to_string(),
                requirement: requirement.to_string(),
            })
    }
}

/// An index rooted at a directory on disk.
///
/// The same document layout an HTTP index serves, which makes this the natural thing
/// to generate into, test against, and ship on media that never touches a network.
#[derive(Debug, Clone)]
pub struct DirectoryIndex {
    base: PathBuf,
    freshness: Freshness,
}

impl DirectoryIndex {
    /// Reads documents from under `base`.
    ///
    /// Defaults to [`Freshness::Lenient`]: a directory you generated yourself has no
    /// separate party who could withhold an update from you.
    pub fn new(base: impl Into<PathBuf>) -> Self {
        DirectoryIndex { base: base.into(), freshness: Freshness::Lenient }
    }

    /// How strictly to treat document expiry.
    pub fn freshness(mut self, freshness: Freshness) -> Self {
        self.freshness = freshness;
        self
    }

    /// The directory this index reads from.
    pub fn base(&self) -> &Path {
        &self.base
    }
}

impl PluginIndex for DirectoryIndex {
    fn releases(&self, name: &str) -> Result<PluginReleases, IndexError> {
        // The name reaches the filesystem, so it is checked before it is joined.
        let name = validate_name(name)?;
        let path = self.base.join(releases_path(name));
        let raw = std::fs::read_to_string(&path).map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => IndexError::UnknownPlugin(name.to_string()),
            _ => IndexError::Transport(format!("{}: {err}", path.display())),
        })?;
        let document = parse_releases(&raw, name)?;
        self.freshness
            .check(&document, &format!("the release document for `{name}`"))?;
        Ok(document)
    }

    fn catalog(&self) -> Result<Catalog, IndexError> {
        let path = self.base.join(CATALOG_PATH);
        let raw = std::fs::read_to_string(&path).map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => IndexError::UnknownPlugin("<catalog>".to_string()),
            _ => IndexError::Transport(format!("{}: {err}", path.display())),
        })?;
        let catalog: Catalog = serde_json::from_str(&raw)
            .map_err(|err| IndexError::Malformed(format!("{}: {err}", path.display())))?;
        self.freshness.check(&catalog, "the catalog")?;
        Ok(catalog)
    }
}

/// Parses and validates a release document.
pub(crate) fn parse_releases(raw: &str, name: &str) -> Result<PluginReleases, IndexError> {
    let document: PluginReleases =
        serde_json::from_str(raw).map_err(|err| IndexError::Malformed(err.to_string()))?;
    document.validate(name)?;
    Ok(document)
}

/// Refuses a plugin name that would escape the document layout.
///
/// Names become path segments in every index implementation, so the check belongs
/// here rather than in each of them.
pub(crate) fn validate_name(name: &str) -> Result<&str, IndexError> {
    let acceptable = !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        && name != "."
        && name != "..";
    if acceptable {
        Ok(name)
    } else {
        Err(IndexError::Malformed(format!(
            "`{name}` is not a usable plugin name: use ASCII letters, digits, `-`, `_` and `.`"
        )))
    }
}

//! Fetching a plugin, checking it, and only then putting it where the registry looks.
//!
//! Installation is staged in three steps that are worth keeping separate, because the
//! interesting decision sits between the second and the third:
//!
//! 1. **Fetch and unpack** into a staging directory beside the plugin root. Nothing
//!    the registry can see has changed yet.
//! 2. **Check** — the digest against the pin, the manifest against the index's claims,
//!    and the new declarations against the installed ones as an
//!    [upgrade review](stanchion_registry::upgrade).
//! 3. **Commit**, which is a rename, or **discard**, which is a delete.
//!
//! [`Staged::commit`] is the only step that touches the plugin root, so a plugin that
//! fails any check never exists there — not briefly, not in a half-written state. The
//! registry is never asked to reason about a directory that is mid-install.
//!
//! ```no_run
//! # use stanchion_dist::{Installer, install::InstallError};
//! # use stanchion_registry::Lockfile;
//! # fn example(installer: &Installer<impl stanchion_dist::PluginIndex, impl stanchion_dist::PluginSource>)
//! # -> Result<(), InstallError> {
//! let mut lockfile = Lockfile::load_or_empty("stanchion.lock")?;
//! let staged = installer.stage("formatter", &"^1.0".parse().unwrap_or_default(), &lockfile)?;
//!
//! if let Some(review) = staged.review() {
//!     if review.widens() {
//!         // A version bump that quietly asks for more authority is the realistic
//!         // attack. Refusing here is the point of the whole arrangement.
//!         for concern in review.concerns() {
//!             eprintln!("  {concern}");
//!         }
//!         staged.discard()?;
//!         return Ok(());
//!     }
//! }
//!
//! let pin = staged.commit()?;
//! lockfile.pin("formatter", pin);
//! lockfile.save("stanchion.lock")?;
//! # Ok(())
//! # }
//! ```

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use semver::VersionReq;
use stanchion_registry::upgrade::UpgradeReview;
use stanchion_registry::{
    DirectoryDigest, LockError, LockedPlugin, Lockfile, Manifest, MANIFEST_FILE,
    read_manifest,
};

use crate::index::{IndexError, PluginIndex, Release};
use crate::package::{self, Limits, PackageError};

/// Why a package could not be fetched.
#[derive(Debug)]
pub enum SourceError {
    /// The reference does not name anything this source can fetch.
    NotFound(String),
    /// The reference is not in a form this source understands.
    Unsupported(String),
    /// The source could not be reached, or answered with an error.
    Transport(String),
    /// The response was not a plugin package.
    Malformed(String),
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SourceError::NotFound(reference) => write!(f, "`{reference}` was not found"),
            SourceError::Unsupported(reference) => {
                write!(f, "`{reference}` is not a reference this source can fetch")
            }
            SourceError::Transport(message) => write!(f, "fetching: {message}"),
            SourceError::Malformed(message) => write!(f, "{message}"),
        }
    }
}

impl Error for SourceError {}

/// Somewhere plugin packages can be fetched from.
///
/// The contract is one method returning bytes, because a source is not trusted with
/// anything else: what it returns is checked against a digest the caller already
/// holds, so a source that lies, a source that is compromised and a source that is
/// merely a stale mirror all fail in the same place, the same way.
pub trait PluginSource {
    /// Fetches the `tar.gz` package named by `reference`.
    fn fetch(&self, reference: &str) -> Result<Vec<u8>, SourceError>;
}

/// Why an install did not complete.
#[derive(Debug)]
pub enum InstallError {
    /// The index could not answer.
    Index(IndexError),
    /// The package could not be fetched.
    Source(SourceError),
    /// The package could not be unpacked, or was not what it claimed to be.
    Package(PackageError),
    /// The release carries no `source` and none was supplied.
    NoSource(String),
    /// The lockfile pins a different build than the index offered.
    Lock(LockError),
    /// The package's manifest disagrees with what the index said about it.
    Mismatch(String),
    /// A file could not be written.
    Io(io::Error),
}

impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InstallError::Index(source) => write!(f, "{source}"),
            InstallError::Source(source) => write!(f, "{source}"),
            InstallError::Package(source) => write!(f, "{source}"),
            InstallError::NoSource(name) => write!(
                f,
                "the index gives no `source` for `{name}`, and no fallback was configured"
            ),
            InstallError::Lock(source) => write!(f, "{source}"),
            InstallError::Mismatch(message) => write!(f, "{message}"),
            InstallError::Io(source) => write!(f, "{source}"),
        }
    }
}

impl Error for InstallError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            InstallError::Index(source) => Some(source),
            InstallError::Source(source) => Some(source),
            InstallError::Package(source) => Some(source),
            InstallError::Lock(source) => Some(source),
            InstallError::Io(source) => Some(source),
            _ => None,
        }
    }
}

impl From<IndexError> for InstallError {
    fn from(source: IndexError) -> Self {
        InstallError::Index(source)
    }
}

impl From<SourceError> for InstallError {
    fn from(source: SourceError) -> Self {
        InstallError::Source(source)
    }
}

impl From<PackageError> for InstallError {
    fn from(source: PackageError) -> Self {
        InstallError::Package(source)
    }
}

impl From<LockError> for InstallError {
    fn from(source: LockError) -> Self {
        InstallError::Lock(source)
    }
}

impl From<io::Error> for InstallError {
    fn from(source: io::Error) -> Self {
        InstallError::Io(source)
    }
}

/// Installs plugins into a root, from an index and a source.
pub struct Installer<I, S> {
    index: I,
    source: S,
    root: PathBuf,
    limits: Limits,
}

impl<I: PluginIndex, S: PluginSource> Installer<I, S> {
    /// Installs into `root`, resolving through `index` and fetching through `source`.
    pub fn new(index: I, source: S, root: impl Into<PathBuf>) -> Self {
        Installer { index, source, root: root.into(), limits: Limits::default() }
    }

    /// Bounds what an archive may expand to.
    pub fn limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// The plugin root being installed into.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The index this installer resolves through.
    pub fn index(&self) -> &I {
        &self.index
    }

    /// Resolves, fetches and checks a plugin without touching the plugin root.
    ///
    /// When `lockfile` already pins this plugin, the pin wins: the exact pinned
    /// version is fetched and the requirement is only used to notice that something
    /// newer exists. An upgrade is therefore always a deliberate act — see
    /// [`Installer::stage_upgrade`].
    pub fn stage(
        &self,
        name: &str,
        requirement: &VersionReq,
        lockfile: &Lockfile,
    ) -> Result<Staged, InstallError> {
        let releases = self.index.releases(name)?;
        let release = match lockfile.get(name).and_then(|pin| pin.version.clone()) {
            Some(pinned) => releases
                .exact(&pinned)
                .ok_or_else(|| {
                    InstallError::Index(IndexError::NoMatch {
                        name: name.to_string(),
                        requirement: format!("={pinned}"),
                    })
                })?
                .clone(),
            None => releases
                .best_match(requirement)
                .cloned()
                .ok_or_else(|| IndexError::NoMatch {
                    name: name.to_string(),
                    requirement: requirement.to_string(),
                })?,
        };
        self.stage_release(name, release, lockfile)
    }

    /// Resolves the newest release satisfying `requirement`, ignoring any pinned
    /// version, and stages it.
    ///
    /// This is how an upgrade is proposed. The staged result carries an
    /// [`UpgradeReview`] against whatever is installed, which is the thing to decide
    /// on before committing.
    pub fn stage_upgrade(
        &self,
        name: &str,
        requirement: &VersionReq,
        lockfile: &Lockfile,
    ) -> Result<Staged, InstallError> {
        let release = self.index.resolve(name, requirement)?;
        self.stage_release(name, release, lockfile)
    }

    /// Fetches and checks one specific release.
    pub fn stage_release(
        &self,
        name: &str,
        release: Release,
        lockfile: &Lockfile,
    ) -> Result<Staged, InstallError> {
        // A pin, when there is one, overrides whatever the index says the digest is.
        // The index is a convenience; the lockfile is the decision.
        if let Some(pin) = lockfile.get(name)
            && !pin.hex().eq_ignore_ascii_case(release.hex())
        {
            return Err(InstallError::Lock(LockError::DigestMismatch {
                name: name.to_string(),
                expected: pin.digest.clone(),
                found: release.digest.clone(),
            }));
        }

        let reference = release
            .source
            .clone()
            .ok_or_else(|| InstallError::NoSource(name.to_string()))?;
        let archive = self.source.fetch(&reference)?;

        let target = self.root.join(name);
        let staging = self.staging_dir(name)?;
        // From here on the staging directory must be cleaned up on every path out,
        // or a failed install leaves rubbish beside the plugin root.
        match self.check_staged(name, &release, &staging) {
            Ok(()) => {}
            Err(err) => {
                let _ = fs::remove_dir_all(&staging);
                return Err(err);
            }
        }
        let outcome = (|| -> Result<Staged, InstallError> {
            package::unpack(archive.as_slice(), &staging, self.limits)?;
            let digest = package::verify_directory(&staging, &release.digest)?;

            let manifest = read_manifest(&staging)
                .map_err(|reason| InstallError::Mismatch(format!("`{name}`: {reason}")))?;
            if manifest.name != name {
                return Err(InstallError::Mismatch(format!(
                    "the package for `{name}` contains a plugin named `{}`",
                    manifest.name
                )));
            }
            if manifest.effective_version() != release.version {
                return Err(InstallError::Mismatch(format!(
                    "`{name}`: the index offers {} but the package declares {}",
                    release.version,
                    manifest.effective_version()
                )));
            }

            let review = read_installed(&target)
                .map(|installed| UpgradeReview::between(&installed, &manifest));

            Ok(Staged {
                name: name.to_string(),
                release,
                digest,
                manifest,
                review,
                staging: staging.clone(),
                target,
            })
        })();

        match outcome {
            Ok(staged) => Ok(staged),
            Err(err) => {
                let _ = fs::remove_dir_all(&staging);
                Err(err)
            }
        }
    }

    /// Checks nothing is in the way before anything is written.
    fn check_staged(
        &self,
        _name: &str,
        _release: &Release,
        staging: &Path,
    ) -> Result<(), InstallError> {
        if fs::read_dir(staging)?.next().is_some() {
            return Err(InstallError::Io(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{} is not empty", staging.display()),
            )));
        }
        Ok(())
    }

    /// A fresh staging directory beside the plugin root.
    ///
    /// Beside, rather than in the system temp directory, so the commit is a rename
    /// within one filesystem rather than a copy that can half-fail.
    fn staging_dir(&self, name: &str) -> Result<PathBuf, InstallError> {
        fs::create_dir_all(&self.root)?;
        let staging = self.root.join(format!(".{name}.staging"));
        if staging.exists() {
            fs::remove_dir_all(&staging)?;
        }
        fs::create_dir(&staging)?;
        fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o700))?;
        Ok(staging)
    }
}

/// Reads an already-installed plugin's manifest, if there is one.
fn read_installed(dir: &Path) -> Option<Manifest> {
    if !dir.join(MANIFEST_FILE).is_file() {
        return None;
    }
    read_manifest(dir).ok()
}

/// A plugin that has been fetched and checked but not yet installed.
///
/// Dropping one without calling [`Staged::commit`] or [`Staged::discard`] leaves the
/// staging directory in place. That is deliberate: a half-finished install is
/// something a host should be able to find afterwards rather than something that
/// silently cleans itself up.
#[derive(Debug)]
pub struct Staged {
    name: String,
    release: Release,
    digest: DirectoryDigest,
    manifest: Manifest,
    review: Option<UpgradeReview>,
    staging: PathBuf,
    target: PathBuf,
}

impl Staged {
    /// The plugin's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The release that was fetched.
    pub fn release(&self) -> &Release {
        &self.release
    }

    /// The manifest inside the package, read without running any of its code.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The verified digest of the staged files.
    pub fn digest(&self) -> &DirectoryDigest {
        &self.digest
    }

    /// What changes relative to the installed version, or `None` on a first install.
    pub fn review(&self) -> Option<&UpgradeReview> {
        self.review.as_ref()
    }

    /// Whether this install asks for authority the installed version did not have.
    ///
    /// A first install always counts as widening: there is nothing to compare against,
    /// so accepting it is a decision, not a continuation of one already made.
    pub fn widens(&self) -> bool {
        self.review.as_ref().is_none_or(UpgradeReview::widens)
    }

    /// Where the files are staged, for a host that wants to look at them.
    pub fn staging_dir(&self) -> &Path {
        &self.staging
    }

    /// Moves the staged plugin into the plugin root and returns the pin to record.
    ///
    /// Any previous version is moved aside and deleted only once the new one is in
    /// place, so an interrupted commit leaves either the old plugin or the new one.
    pub fn commit(self) -> Result<LockedPlugin, InstallError> {
        let previous = self.target.with_extension("previous");
        if previous.exists() {
            fs::remove_dir_all(&previous)?;
        }
        let had_previous = self.target.exists();
        if had_previous {
            fs::rename(&self.target, &previous)?;
        }

        match fs::rename(&self.staging, &self.target) {
            Ok(()) => {}
            Err(err) => {
                // Put the old version back rather than leaving the host with neither.
                if had_previous {
                    let _ = fs::rename(&previous, &self.target);
                }
                let _ = fs::remove_dir_all(&self.staging);
                return Err(InstallError::Io(err));
            }
        }
        if had_previous {
            let _ = fs::remove_dir_all(&previous);
        }

        let mut pin = LockedPlugin::from_digest(&self.digest)
            .at_version(self.manifest.effective_version());
        if let Some(signer) = &self.release.signer {
            pin = pin.signed_by(signer.clone());
        }
        if let Some(issuer) = &self.release.issuer {
            pin = pin.issued_by(issuer.clone());
        }
        if let Some(source) = &self.release.source {
            pin = pin.fetched_from(source.clone());
        }
        Ok(pin)
    }

    /// Deletes the staged files, leaving the installed version untouched.
    pub fn discard(self) -> Result<(), InstallError> {
        fs::remove_dir_all(&self.staging)?;
        Ok(())
    }
}

//! The wire format: a plugin directory as a `tar.gz`, and the rules for unpacking one.
//!
//! # Why the archive is not the pin
//!
//! A pin is a [`DirectoryDigest`](stanchion_registry::DirectoryDigest) over the
//! *unpacked directory* — paths and file contents, nothing else — not a hash of the
//! archive bytes. Repacking a plugin with different mtimes, a different entry order or
//! a different gzip level produces different bytes and the same pin.
//!
//! That is deliberate, and it cuts both ways:
//!
//! - A mirror can repack without invalidating a signature, so mirroring needs no
//!   trust and no re-signing.
//! - **Tar metadata is not covered by any signature here.** Modes, owners, mtimes and
//!   entry types are attacker-controlled even for a correctly signed plugin, so
//!   [`unpack`] ignores all of them rather than honouring them.
//!
//! # What unpacking refuses
//!
//! An archive arriving from a registry is hostile input, and it is hostile *before*
//! anything about it has been verified — the digest cannot be checked until the files
//! are on disk. So extraction refuses anything that could write outside the
//! destination or exhaust the machine:
//!
//! - absolute paths, `..` components, and paths with a root or drive prefix
//! - symlinks and hard links, of any target
//! - anything that is not a regular file or a directory
//! - the same path appearing twice
//! - more than [`Limits::entries`] files or [`Limits::total_bytes`] uncompressed
//!
//! Symlinks are refused rather than sanitised because a symlink is not representable
//! in a `DirectoryDigest` in the first place: the digest hashes what
//! [`std::fs::read`] returns, which for a symlink is whatever it points at on the
//! machine doing the hashing. A plugin containing one would verify on the signer's
//! machine and mean something different on yours.

use std::error::Error;
use std::fmt;
use std::io;
use std::path::{Component, Path};

#[cfg(feature = "package")]
use std::collections::BTreeSet;
#[cfg(feature = "package")]
use std::fs;
#[cfg(feature = "package")]
use std::io::{Read, Write};
#[cfg(feature = "package")]
use std::path::PathBuf;

use stanchion_registry::DirectoryDigest;

/// Media type of a plugin package, for servers that want to label one.
pub const PACKAGE_MEDIA_TYPE: &str = "application/vnd.stanchion.plugin.v1.tar+gzip";

/// Why a package could not be produced or unpacked.
#[derive(Debug)]
pub enum PackageError {
    /// The archive holds an entry that could write outside the destination.
    UnsafePath(String),
    /// The archive holds something that is not a regular file or directory.
    UnsafeEntry {
        /// The entry's path, as the archive spells it.
        path: String,
        /// What kind of thing it is.
        kind: String,
    },
    /// The archive lists one path more than once.
    DuplicateEntry(String),
    /// The archive is larger than the configured limits allow.
    TooLarge(String),
    /// The unpacked directory is not the plugin it claimed to be.
    DigestMismatch {
        /// The digest that was expected.
        expected: String,
        /// The digest the unpacked files produced.
        found: String,
    },
    /// The archive could not be read or written.
    Io(io::Error),
}

impl fmt::Display for PackageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PackageError::UnsafePath(path) => write!(
                f,
                "`{path}` would be written outside the destination directory"
            ),
            PackageError::UnsafeEntry { path, kind } => {
                write!(f, "`{path}` is a {kind}, which a plugin package may not contain")
            }
            PackageError::DuplicateEntry(path) => {
                write!(f, "`{path}` appears more than once in the archive")
            }
            PackageError::TooLarge(message) => write!(f, "{message}"),
            PackageError::DigestMismatch { expected, found } => write!(
                f,
                "these bytes are {found}, not the expected {expected}"
            ),
            PackageError::Io(source) => write!(f, "{source}"),
        }
    }
}

impl Error for PackageError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            PackageError::Io(source) => Some(source),
            _ => None,
        }
    }
}

impl From<io::Error> for PackageError {
    fn from(source: io::Error) -> Self {
        PackageError::Io(source)
    }
}

/// Bounds on what an archive may expand to.
///
/// A compressed archive can expand by orders of magnitude, and the only sound moment
/// to stop is while writing rather than after.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Most files an archive may contain.
    pub entries: usize,
    /// Most uncompressed bytes an archive may expand to, in total.
    pub total_bytes: u64,
    /// Most uncompressed bytes any single file may expand to.
    pub file_bytes: u64,
}

impl Default for Limits {
    /// Generous for a plugin, ruinous for a decompression bomb.
    fn default() -> Self {
        Limits {
            entries: 4096,
            total_bytes: 64 * 1024 * 1024,
            file_bytes: 16 * 1024 * 1024,
        }
    }
}

/// Writes a plugin directory to a `tar.gz`.
///
/// Entry metadata is normalised — fixed mtime, mode and ownership — so packing the
/// same directory twice produces the same bytes. Nothing depends on that, since the
/// pin is over the directory rather than the archive, but a reproducible package is
/// far easier to reason about when two of them disagree.
#[cfg(feature = "package")]
pub fn pack(dir: &Path, out: impl Write) -> Result<DirectoryDigest, PackageError> {
    let digest = DirectoryDigest::compute(dir)?;

    let encoder = flate2::write::GzEncoder::new(out, flate2::Compression::default());
    let mut archive = tar::Builder::new(encoder);
    archive.follow_symlinks(false);

    let mut paths: Vec<(String, PathBuf)> = Vec::new();
    collect(dir, dir, &mut paths)?;
    // Sorted, so the archive's order matches the digest's and repacking is stable.
    paths.sort_by(|left, right| left.0.cmp(&right.0));

    for (relative, path) in paths {
        let contents = fs::read(&path)?;
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mode(0o644);
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        archive
            .append_data(&mut header, &relative, contents.as_slice())
            .map_err(PackageError::Io)?;
    }

    archive
        .into_inner()
        .map_err(PackageError::Io)?
        .finish()
        .map_err(PackageError::Io)?;
    Ok(digest)
}

/// Walks a plugin directory, refusing anything that is not a regular file.
#[cfg(feature = "package")]
fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<(), PackageError> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_dir() {
            collect(root, &path, out)?;
            continue;
        }
        if !kind.is_file() {
            return Err(PackageError::UnsafeEntry {
                path: path.display().to_string(),
                kind: describe(&kind),
            });
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| PackageError::UnsafePath(path.display().to_string()))?
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        out.push((relative, path));
    }
    Ok(())
}

/// Names a file type for an error a person has to act on.
#[cfg(feature = "package")]
fn describe(kind: &fs::FileType) -> String {
    if kind.is_symlink() {
        "symlink".to_string()
    } else {
        format!("{kind:?}")
    }
}

/// Extracts a `tar.gz` into an empty directory, refusing anything unsafe.
///
/// `into` must exist and be empty: unpacking over a populated directory would leave
/// files the archive did not contain, and those files are covered by the digest that
/// gets checked next.
#[cfg(feature = "package")]
pub fn unpack(archive: impl Read, into: &Path, limits: Limits) -> Result<(), PackageError> {
    let decoder = flate2::read::GzDecoder::new(archive);
    let mut archive = tar::Archive::new(decoder);
    // Belt and braces: the path checks below are what actually protect the
    // destination, but these stop the tar crate from ever attempting the write.
    archive.set_preserve_permissions(false);
    archive.set_unpack_xattrs(false);
    archive.set_overwrite(false);

    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut total: u64 = 0;

    for entry in archive.entries()? {
        let mut entry = entry?;
        let header = entry.header().clone();
        let raw = entry.path()?.to_path_buf();
        let display = raw.display().to_string();

        let kind = header.entry_type();
        if kind.is_dir() {
            // Directories are created as their files need them, so an explicit entry
            // only has to be checked, not acted on.
            safe_relative(&raw).ok_or_else(|| PackageError::UnsafePath(display.clone()))?;
            continue;
        }
        if !kind.is_file() {
            return Err(PackageError::UnsafeEntry {
                path: display,
                kind: entry_kind(kind),
            });
        }

        let relative =
            safe_relative(&raw).ok_or_else(|| PackageError::UnsafePath(display.clone()))?;
        if !seen.insert(relative.clone()) {
            return Err(PackageError::DuplicateEntry(relative));
        }
        if seen.len() > limits.entries {
            return Err(PackageError::TooLarge(format!(
                "archive holds more than {} entries",
                limits.entries
            )));
        }

        let declared = header.size().unwrap_or(0);
        if declared > limits.file_bytes {
            return Err(PackageError::TooLarge(format!(
                "`{relative}` declares {declared} bytes, over the {} byte limit",
                limits.file_bytes
            )));
        }
        total = total.saturating_add(declared);
        if total > limits.total_bytes {
            return Err(PackageError::TooLarge(format!(
                "archive expands past the {} byte limit",
                limits.total_bytes
            )));
        }

        let destination = into.join(&relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }

        // Copy through a bounded reader rather than trusting the declared size: a
        // header can understate what the stream actually carries.
        let mut file = fs::File::create(&destination)?;
        let budget = limits.file_bytes.saturating_add(1);
        let written = io::copy(&mut entry.by_ref().take(budget), &mut file)?;
        if written > limits.file_bytes {
            return Err(PackageError::TooLarge(format!(
                "`{relative}` is larger than its header declared"
            )));
        }
        file.flush()?;
    }
    Ok(())
}

/// Names a tar entry type for an error a person has to act on.
#[cfg(feature = "package")]
fn entry_kind(kind: tar::EntryType) -> String {
    match kind {
        tar::EntryType::Symlink => "symlink".to_string(),
        tar::EntryType::Link => "hard link".to_string(),
        tar::EntryType::Char => "character device".to_string(),
        tar::EntryType::Block => "block device".to_string(),
        tar::EntryType::Fifo => "fifo".to_string(),
        other => format!("{other:?} entry"),
    }
}

/// Reduces an archive path to a safe `/`-separated relative path, or refuses it.
///
/// Refuses a root, a drive prefix, any `..`, and anything that normalises to nothing.
/// `.` components are dropped because tar writers emit them routinely and they cannot
/// escape anything.
#[cfg_attr(not(feature = "package"), allow(dead_code))]
pub(crate) fn safe_relative(path: &Path) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let part = part.to_str()?;
                if part.is_empty() {
                    return None;
                }
                parts.push(part.to_string());
            }
            Component::CurDir => {}
            // A parent, root or prefix component is the whole attack; there is no
            // sanitised form of one worth accepting.
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/"))
}

/// Confirms an unpacked directory is the plugin a digest names.
pub fn verify_directory(dir: &Path, expected: &str) -> Result<DirectoryDigest, PackageError> {
    let digest = DirectoryDigest::compute(dir)?;
    let expected_hex = expected.strip_prefix("sha256:").unwrap_or(expected);
    let found = digest.hex();
    if !expected_hex.eq_ignore_ascii_case(&found) {
        return Err(PackageError::DigestMismatch {
            expected: format!("sha256:{}", expected_hex.to_ascii_lowercase()),
            found: format!("sha256:{found}"),
        });
    }
    Ok(digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traversal_and_absolute_paths_are_refused() {
        assert_eq!(safe_relative(Path::new("init.lua")).as_deref(), Some("init.lua"));
        assert_eq!(
            safe_relative(Path::new("./lib/./util.lua")).as_deref(),
            Some("lib/util.lua")
        );
        assert_eq!(safe_relative(Path::new("../outside")), None);
        assert_eq!(safe_relative(Path::new("lib/../../outside")), None);
        assert_eq!(safe_relative(Path::new("/etc/passwd")), None);
        assert_eq!(safe_relative(Path::new("")), None);
        assert_eq!(safe_relative(Path::new(".")), None);
    }
}

//! Where a server gets documents and packages.

use std::io::Read;
use std::path::{Path, PathBuf};

use stanchion_dist::{Catalog, DirectoryIndex, Freshness, IndexError, PluginIndex, PluginReleases};

/// URL prefix under which packages are served, relative to the index base.
pub const BLOBS_PREFIX: &str = "v1/blobs/";

/// Directory holding packages on disk, relative to a [`DirectorySource`] base.
///
/// A path segment rather than the URL's `sha256:<hex>`, because a colon in a filename
/// is legal on Linux and a nuisance everywhere else.
pub const BLOBS_SUBDIR: &str = "blobs";

/// Whatever a server answers requests from.
///
/// Deliberately the same shape as [`PluginIndex`] plus packages. Documents come back
/// parsed rather than as bytes, because the server rewrites `updated` and `expires`
/// before serialising — the whole reason to run one.
pub trait IndexSource {
    /// The releases of one plugin. The name has already been validated.
    fn releases(&self, name: &str) -> Result<PluginReleases, IndexError>;

    /// The catalog, if this source can enumerate.
    fn catalog(&self) -> Result<Catalog, IndexError> {
        Err(IndexError::UnknownPlugin("<catalog>".to_string()))
    }

    /// One package, by its `sha256:<hex>` digest.
    ///
    /// The digest is the *unpacked directory's*, which is what a client verifies after
    /// extracting — not a hash of these bytes. A source therefore looks the archive up
    /// by that digest rather than computing one.
    ///
    /// Returns a reader rather than bytes so a package never has to be held in memory
    /// to be served. A source that genuinely has bytes can use [`Blob::from_bytes`].
    fn blob(&self, digest: &str) -> Result<Blob, IndexError>;
}

/// A package, ready to be streamed.
pub struct Blob {
    /// The archive's bytes, read on demand.
    pub reader: Box<dyn Read + Send>,
    /// Length, where the source knows it, for `Content-Length`.
    pub len: Option<u64>,
}

impl Blob {
    /// A package a source already holds in memory.
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        let len = bytes.len() as u64;
        Blob { reader: Box::new(std::io::Cursor::new(bytes)), len: Some(len) }
    }

    /// A package read from anywhere, with a length if it is known.
    pub fn from_reader(reader: impl Read + Send + 'static, len: Option<u64>) -> Self {
        Blob { reader: Box::new(reader), len }
    }
}

/// Serves an index laid out on disk.
///
/// ```text
/// <base>/v1/index.json              the catalog, optional
/// <base>/v1/plugins/<name>.json     one plugin's releases
/// <base>/blobs/sha256/<hex>         the package for that digest
/// ```
///
/// Stored documents are read with [`Freshness::Ignored`]: their own `expires` is
/// irrelevant because the server issues a new one on every response. A directory of
/// documents that would be too stale for a client to accept is still a perfectly good
/// source for a server that keeps them fresh.
pub struct DirectorySource {
    base: PathBuf,
    index: DirectoryIndex,
}

impl DirectorySource {
    /// Reads the layout above from `base`.
    pub fn new(base: impl Into<PathBuf>) -> Self {
        let base = base.into();
        let index = DirectoryIndex::new(base.clone()).freshness(Freshness::Ignored);
        DirectorySource { base, index }
    }

    /// The directory being served.
    pub fn base(&self) -> &Path {
        &self.base
    }

    /// On-disk path of one package.
    fn blob_path(&self, digest: &str) -> Option<PathBuf> {
        // `sha256:<64 hex>` and nothing else: the hex becomes a filename.
        let hex = digest.strip_prefix("sha256:")?;
        if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        Some(
            self.base
                .join(BLOBS_SUBDIR)
                .join("sha256")
                .join(hex.to_ascii_lowercase()),
        )
    }
}

impl IndexSource for DirectorySource {
    fn releases(&self, name: &str) -> Result<PluginReleases, IndexError> {
        self.index.releases(name)
    }

    fn catalog(&self) -> Result<Catalog, IndexError> {
        self.index.catalog()
    }

    fn blob(&self, digest: &str) -> Result<Blob, IndexError> {
        let path = self
            .blob_path(digest)
            .ok_or_else(|| IndexError::Malformed(format!("`{digest}` is not a sha256 digest")))?;

        // Opened, not read: the file is streamed to the socket, so a large package
        // costs a buffer rather than its own size in memory.
        let file = std::fs::File::open(&path).map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => IndexError::UnknownPlugin(digest.to_string()),
            _ => IndexError::Transport(format!("{}: {err}", path.display())),
        })?;
        let len = file.metadata().ok().map(|meta| meta.len());
        Ok(Blob::from_reader(file, len))
    }
}

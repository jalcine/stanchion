//! Shared publish-side logic for the registry example.
//!
//! Both `registry-publish` (packing a local directory) and the upload endpoint in
//! `registry-serve` (promoting bytes that arrived over the network) end here, after
//! their own checks. Promotion itself is deliberately dumb: it files the archive
//! under its verified digest and records the release. It never decides trust — the
//! callers establish the digest and the signer before calling.

use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use semver::Version;
use stanchion_dist::{
    CATALOG_PATH, Catalog, INDEX_SCHEMA, PluginReleases, Release, releases_path, validate_name,
};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// Directory holding packages, mirroring [`stanchion_index::BLOBS_SUBDIR`] layout.
const BLOBS_DIR: &str = "blobs/sha256";

/// Why a package could not be promoted.
#[derive(Debug)]
pub struct PromoteError(pub String);

impl fmt::Display for PromoteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for PromoteError {}

/// The content directory layout this example generates and serves.
#[derive(Debug, Clone)]
pub struct ContentDir {
    base: PathBuf,
}

impl ContentDir {
    /// The directory holding the index documents and packages.
    pub fn new(base: impl Into<PathBuf>) -> Self {
        ContentDir { base: base.into() }
    }

    /// The directory being served.
    pub fn path(&self) -> &Path {
        &self.base
    }

    /// On-disk path of one package, by bare hex digest.
    pub fn blob_path(&self, hex: &str) -> Result<PathBuf, PromoteError> {
        if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(PromoteError(format!("`{hex}` is not a sha256 digest")));
        }
        Ok(self.base.join(BLOBS_DIR).join(hex.to_ascii_lowercase()))
    }
}

/// Files one package under its digest and records the release.
///
/// `digest_hex` must already be verified — against the directory `publish` packed,
/// or against the staging directory the upload endpoint unpacked into. This
/// function checks the shape of the name and version, never the trust behind them.
#[allow(clippy::too_many_arguments)]
pub fn promote(
    content: &ContentDir,
    name: &str,
    version: Version,
    signer: Option<&str>,
    digest_hex: &str,
    archive: &[u8],
    source_base: &str,
    expires_days: i64,
) -> Result<Release, PromoteError> {
    let name = validate_name(name).map_err(|err| PromoteError(err.to_string()))?;
    let blob = content.blob_path(digest_hex)?;
    let digest = format!("sha256:{digest_hex}");

    write_atomic(&blob, archive)?;

    let source = format!(
        "{}/{BLOBS_DIR_URL}{digest}",
        source_base.trim_end_matches('/')
    );
    let mut release = Release::new(version, digest.clone()).from_source(source);
    if let Some(signer) = signer {
        release = release.signed_by(signer);
    }

    let releases_path = content.base.join(releases_path(name));
    let mut document = read_releases(&releases_path, name)?;
    document
        .releases
        .retain(|other| other.version != release.version);
    document.releases.push(release.clone());
    document
        .releases
        .sort_by(|left, right| left.version.cmp(&right.version));
    document.schema = INDEX_SCHEMA;
    let (updated, expires) = window(expires_days);
    document.updated = Some(updated.clone());
    document.expires = Some(expires.clone());
    write_atomic(
        &releases_path,
        serde_json::to_vec_pretty(&document)
            .map_err(|err| PromoteError(format!("rendering releases: {err}")))?,
    )?;

    let catalog_path = content.base.join(CATALOG_PATH);
    let mut catalog = read_catalog(&catalog_path)?;
    if !catalog.plugins.iter().any(|other| other == name) {
        catalog.plugins.push(name.to_string());
        catalog.plugins.sort();
    }
    catalog.schema = INDEX_SCHEMA;
    catalog.updated = Some(updated);
    catalog.expires = Some(expires);
    write_atomic(
        &catalog_path,
        serde_json::to_vec_pretty(&catalog)
            .map_err(|err| PromoteError(format!("rendering catalog: {err}")))?,
    )?;

    Ok(release)
}

/// `updated` now and `expires` a number of days out, as RFC 3339.
pub fn window(expires_days: i64) -> (String, String) {
    let now = OffsetDateTime::now_utc();
    let days = expires_days.clamp(1, 365);
    let expires = now.saturating_add(time::Duration::days(days));
    (
        now.format(&Rfc3339).unwrap_or_default(),
        expires.format(&Rfc3339).unwrap_or_default(),
    )
}

/// Human-readable size, for log lines.
pub fn display_bytes(len: usize) -> String {
    const UNIT: usize = 1024;
    if len < UNIT {
        return format!("{len} B");
    }
    let kib = len / UNIT;
    if kib < UNIT {
        return format!("{kib} KiB");
    }
    format!("{} MiB", kib / UNIT)
}

const BLOBS_DIR_URL: &str = "v1/blobs/";

fn read_releases(path: &Path, name: &str) -> Result<PluginReleases, PromoteError> {
    match fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw)
            .map_err(|err| PromoteError(format!("{}: {err}", path.display()))),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(PluginReleases {
            schema: INDEX_SCHEMA,
            name: name.to_string(),
            updated: None,
            expires: None,
            releases: Vec::new(),
        }),
        Err(err) => Err(PromoteError(format!("{}: {err}", path.display()))),
    }
}

fn read_catalog(path: &Path) -> Result<Catalog, PromoteError> {
    match fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw)
            .map_err(|err| PromoteError(format!("{}: {err}", path.display()))),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Catalog {
            schema: INDEX_SCHEMA,
            name: Some("example registry".to_string()),
            updated: None,
            expires: None,
            plugins: Vec::new(),
        }),
        Err(err) => Err(PromoteError(format!("{}: {err}", path.display()))),
    }
}

/// Writes through a temporary sibling and renames, so a crash leaves the previous
/// document rather than half of a new one.
fn write_atomic(path: &Path, bytes: impl AsRef<[u8]>) -> Result<(), PromoteError> {
    let parent = path
        .parent()
        .ok_or_else(|| PromoteError(format!("{} has no parent directory", path.display())))?;
    fs::create_dir_all(parent)
        .map_err(|err| PromoteError(format!("{}: {err}", parent.display())))?;
    let mut staging = path.as_os_str().to_owned();
    staging.push(".staging");
    let staging = PathBuf::from(staging);
    fs::write(&staging, bytes.as_ref())
        .map_err(|err| PromoteError(format!("{}: {err}", staging.display())))?;
    fs::rename(&staging, path).map_err(|err| PromoteError(format!("{}: {err}", path.display())))?;
    Ok(())
}

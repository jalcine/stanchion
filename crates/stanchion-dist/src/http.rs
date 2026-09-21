//! Reading an index over HTTPS.
//!
//! The index is [two static JSON documents](crate::index), so this is a GET and a
//! parse. There is no API to speak of, which is the point: anything that serves files
//! can host an index, and this client works against all of it.
//!
//! # Transport security is not the security
//!
//! TLS authenticates the *server*, which is a claim about who you are talking to and
//! not about what they said. An index reached over a perfect TLS connection to a
//! compromised bucket is a compromised index. What actually protects a host is that
//! nothing the index says is believed: the digest it hands over is checked against the
//! bytes that arrive, the [pin](stanchion_registry::lock) is what the registry
//! enforces, and a document past its [expiry](crate::Freshness) is refused outright.
//!
//! So HTTPS here is a courtesy — it keeps a passive network from seeing which plugins
//! a host runs — rather than the thing standing between a host and a bad plugin.

use std::time::Duration;

use crate::install::{PluginSource, SourceError};
use crate::index::{
    Catalog, Freshness, IndexError, PluginIndex, PluginReleases, CATALOG_PATH, releases_path,
    validate_name,
};

/// An index served over HTTP.
pub struct HttpIndex {
    client: reqwest::blocking::Client,
    base: String,
    freshness: Freshness,
}

impl HttpIndex {
    /// Reads documents from under `base`, which may or may not end in a slash.
    ///
    /// Defaults to [`Freshness::Required`]: a document fetched from somewhere else is
    /// exactly the case where a withheld update has to be noticed, so an index without
    /// an expiry is refused rather than believed indefinitely.
    pub fn new(base: impl Into<String>) -> Result<Self, IndexError> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(concat!("stanchion-dist/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|err| IndexError::Transport(err.to_string()))?;
        Ok(HttpIndex {
            client,
            base: base.into().trim_end_matches('/').to_string(),
            freshness: Freshness::Required,
        })
    }

    /// Uses a caller-configured client, for proxies, client certificates or pinning.
    pub fn with_client(client: reqwest::blocking::Client, base: impl Into<String>) -> Self {
        HttpIndex {
            client,
            base: base.into().trim_end_matches('/').to_string(),
            freshness: Freshness::Required,
        }
    }

    /// How strictly to treat document expiry.
    pub fn freshness(mut self, freshness: Freshness) -> Self {
        self.freshness = freshness;
        self
    }

    /// Fetches one document, mapping 404 to a missing plugin rather than an outage.
    fn get(&self, path: &str, what: &str) -> Result<String, IndexError> {
        let url = format!("{}/{path}", self.base);
        let response = self
            .client
            .get(&url)
            .send()
            .map_err(|err| IndexError::Transport(format!("{url}: {err}")))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(IndexError::UnknownPlugin(what.to_string()));
        }
        if !response.status().is_success() {
            return Err(IndexError::Transport(format!(
                "{url}: the index answered {}",
                response.status()
            )));
        }
        response
            .text()
            .map_err(|err| IndexError::Transport(format!("{url}: {err}")))
    }
}

impl PluginIndex for HttpIndex {
    fn releases(&self, name: &str) -> Result<PluginReleases, IndexError> {
        // The name becomes part of a URL, so it is checked before it is interpolated.
        let name = validate_name(name)?;
        let raw = self.get(&releases_path(name), name)?;
        let document = crate::index::parse_releases(&raw, name)?;
        self.freshness
            .check(&document, &format!("the release document for `{name}`"))?;
        Ok(document)
    }

    fn catalog(&self) -> Result<Catalog, IndexError> {
        let raw = self.get(CATALOG_PATH, "<catalog>")?;
        let catalog: Catalog = serde_json::from_str(&raw)
            .map_err(|err| IndexError::Malformed(err.to_string()))?;
        self.freshness.check(&catalog, "the catalog")?;
        Ok(catalog)
    }
}

/// Fetches plugin packages over HTTPS.
///
/// A plugin registry needs one thing of a package source: given a reference, return
/// the archive bytes. There is no manifest to resolve and no tag to dereference,
/// because the index already answered that question — a release names its digest, and
/// the digest is what the caller verifies after unpacking.
///
/// That makes a package URL an ordinary immutable file. Anything that serves bytes can
/// host one: a bucket, a CDN, `nginx`, or the index server itself.
pub struct HttpSource {
    client: reqwest::blocking::Client,
    limit: u64,
}

/// Largest package this source will hold in memory, unless raised.
pub const DEFAULT_PACKAGE_LIMIT: u64 = 64 * 1024 * 1024;

impl HttpSource {
    /// A source with a default client and package-size cap.
    pub fn new() -> Self {
        HttpSource {
            client: reqwest::blocking::Client::new(),
            limit: DEFAULT_PACKAGE_LIMIT,
        }
    }

    /// A source using a caller-supplied client, for proxies, timeouts or pinned roots.
    pub fn with_client(client: reqwest::blocking::Client) -> Self {
        HttpSource { client, limit: DEFAULT_PACKAGE_LIMIT }
    }

    /// Caps how large a package may be.
    ///
    /// `fetch` returns the archive in memory, so without a cap a hostile or misbehaving
    /// server could exhaust it. This bounds the download; [`crate::Limits`] bounds what
    /// unpacking it may produce.
    pub fn limit(mut self, bytes: u64) -> Self {
        self.limit = bytes;
        self
    }
}

impl Default for HttpSource {
    fn default() -> Self {
        HttpSource::new()
    }
}

impl PluginSource for HttpSource {
    fn fetch(&self, reference: &str) -> Result<Vec<u8>, SourceError> {
        // Another source may understand a scheme this one does not, so an unknown
        // scheme is `Unsupported` rather than a failure.
        if !(reference.starts_with("https://") || reference.starts_with("http://")) {
            return Err(SourceError::Unsupported(reference.to_string()));
        }

        let response = self
            .client
            .get(reference)
            .send()
            .map_err(|err| SourceError::Transport(format!("{reference}: {err}")))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(SourceError::NotFound(reference.to_string()));
        }
        if !response.status().is_success() {
            return Err(SourceError::Transport(format!(
                "{reference}: the source answered {}",
                response.status()
            )));
        }

        // Refuse an oversized package on the declared length where there is one, so an
        // obvious case costs nothing to reject.
        if let Some(length) = response.content_length()
            && length > self.limit
        {
            return Err(SourceError::Malformed(format!(
                "{reference}: the package declares {length} bytes, over the {} byte limit",
                self.limit
            )));
        }

        // A declared length is a claim, so the body is capped as it is read too.
        let mut body = Vec::new();
        // `Read::take`, not `Iterator::take`: a Response is both.
        let mut reader = std::io::Read::take(response, self.limit.saturating_add(1));
        std::io::Read::read_to_end(&mut reader, &mut body)
            .map_err(|err| SourceError::Transport(format!("{reference}: {err}")))?;

        if body.len() as u64 > self.limit {
            return Err(SourceError::Malformed(format!(
                "{reference}: the package exceeds the {} byte limit",
                self.limit
            )));
        }
        Ok(body)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    /// A reference this source does not understand must be `Unsupported`, not a
    /// failure: an `Installer` may have another source that does understand it.
    #[test]
    fn a_foreign_scheme_is_unsupported_rather_than_an_error() {
        let source = HttpSource::new();
        for reference in ["oci://ghcr.io/acme/p:1", "ftp://host/p.tar.gz", "p.tar.gz", ""] {
            match source.fetch(reference) {
                Err(SourceError::Unsupported(named)) => assert_eq!(named, reference),
                other => panic!("expected Unsupported for `{reference}`, got {other:?}"),
            }
        }
    }

    #[test]
    fn the_package_limit_is_configurable_and_defaulted() {
        assert_eq!(HttpSource::new().limit, DEFAULT_PACKAGE_LIMIT);
        assert_eq!(HttpSource::new().limit(1024).limit, 1024);
    }
}

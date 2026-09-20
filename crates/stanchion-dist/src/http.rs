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

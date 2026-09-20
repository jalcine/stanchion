//! Fetching and publishing plugin packages as OCI artifacts.
//!
//! An OCI registry is a content-addressed blob store with authentication, mirroring,
//! retention and access control already solved, available from every cloud and
//! runnable locally in one command. Using one means a host operating a plugin
//! ecosystem does not also have to operate a package server.
//!
//! # The artifact
//!
//! ```text
//! manifest  artifactType: application/vnd.stanchion.plugin.v1+json
//!   config  application/vnd.stanchion.plugin.config.v1+json   the manifest's metadata
//!   layer   application/vnd.stanchion.plugin.layer.v1.tar+gzip the plugin directory
//! ```
//!
//! One layer, holding the plugin directory — including its `plugin.sigstore.json`,
//! which the [`DirectoryDigest`](stanchion_registry::DirectoryDigest) excludes from
//! itself precisely so a package can carry its own signature.
//!
//! # The registry's digest is not the pin
//!
//! OCI addresses the compressed tarball. Stanchion addresses the directory. They are
//! different numbers over different things, and only the second one is pinned.
//!
//! That is not a weakness, it is the reason mirroring is safe: a mirror that repacks,
//! recompresses or re-tags changes every OCI digest in sight and cannot alter the
//! directory digest without altering a file, which is what the signature covers. A
//! `@sha256:` reference is still worth using when you have one — it saves a round trip
//! and fails earlier — but nothing rests on it.
//!
//! # What the registry is trusted with
//!
//! Nothing. It can serve the wrong bytes, an old version, or nothing at all. The
//! first two fail at the digest check in [`install`](crate::install); the third is
//! indistinguishable from the registry being down, which no amount of cryptography
//! fixes.

use std::collections::BTreeMap;
use std::str::FromStr;

use oci_client::client::{ClientConfig, Config, ImageLayer};
use oci_client::manifest::OciImageManifest;
use oci_client::secrets::RegistryAuth;
use oci_client::{Client, Reference};
use serde::{Deserialize, Serialize};

use crate::install::{PluginSource, SourceError};
use crate::package::{
    ARTIFACT_TYPE, CONFIG_MEDIA_TYPE, DIGEST_ANNOTATION, LAYER_MEDIA_TYPE, NAME_ANNOTATION,
    VERSION_ANNOTATION,
};

/// Scheme this crate uses in an index's `source` field.
pub const OCI_SCHEME: &str = "oci://";

/// The artifact config blob: what a registry UI can show without pulling the layer.
///
/// Advisory, like everything outside the layer. The manifest inside the package is
/// what the registry reads; this exists so a person browsing a registry can see what
/// a plugin is without downloading and unpacking it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactConfig {
    /// Plugin name.
    pub name: String,
    /// Plugin version.
    pub version: String,
    /// Root directory digest, as `sha256:<hex>`.
    pub digest: String,
    /// Capabilities the plugin's manifest declares.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
}

/// Fetches plugin packages from OCI registries.
pub struct OciSource {
    client: Client,
    auth: RegistryAuth,
}

impl Default for OciSource {
    fn default() -> Self {
        OciSource::anonymous()
    }
}

impl OciSource {
    /// Pulls anonymously, which is what a public registry needs.
    pub fn anonymous() -> Self {
        OciSource { client: Client::new(ClientConfig::default()), auth: RegistryAuth::Anonymous }
    }

    /// Pulls with credentials.
    pub fn authenticated(auth: RegistryAuth) -> Self {
        OciSource { client: Client::new(ClientConfig::default()), auth }
    }

    /// Uses a caller-configured client, for a registry needing particular TLS or
    /// protocol settings.
    pub fn with_client(client: Client, auth: RegistryAuth) -> Self {
        OciSource { client, auth }
    }

    /// Publishes a packed plugin as an OCI artifact.
    ///
    /// `archive` is the `tar.gz` from [`pack`](crate::package::pack) and `config`
    /// describes what is inside it. Returns the manifest URL the registry reports.
    pub fn publish(
        &self,
        reference: &str,
        archive: Vec<u8>,
        config: &ArtifactConfig,
    ) -> Result<String, SourceError> {
        let target = parse_reference(reference)?;
        let annotations = BTreeMap::from([
            (NAME_ANNOTATION.to_string(), config.name.clone()),
            (VERSION_ANNOTATION.to_string(), config.version.clone()),
            (DIGEST_ANNOTATION.to_string(), config.digest.clone()),
        ]);

        let layer = ImageLayer::new(
            archive,
            LAYER_MEDIA_TYPE.to_string(),
            Some(annotations.clone()),
        );
        let raw_config = serde_json::to_vec(config)
            .map_err(|err| SourceError::Malformed(format!("describing the artifact: {err}")))?;
        let config_blob = Config::new(
            raw_config,
            CONFIG_MEDIA_TYPE.to_string(),
            Some(annotations.clone()),
        );

        let layers = vec![layer];
        let mut manifest = OciImageManifest::build(&layers, &config_blob, Some(annotations));
        manifest.artifact_type = Some(ARTIFACT_TYPE.to_string());

        let response = block_on(|| async {
            self.client
                .push(&target, &layers, config_blob, &self.auth, Some(manifest))
                .await
        })?;
        Ok(response.manifest_url)
    }
}

impl PluginSource for OciSource {
    fn fetch(&self, reference: &str) -> Result<Vec<u8>, SourceError> {
        let target = parse_reference(reference)?;

        let image = block_on(|| async {
            self.client
                .pull(
                    &target,
                    &self.auth,
                    // Only the layer type this crate publishes. A registry answering
                    // with something else is answering a different question.
                    vec![LAYER_MEDIA_TYPE],
                )
                .await
        })?;

        let mut layers = image
            .layers
            .into_iter()
            .filter(|layer| layer.media_type == LAYER_MEDIA_TYPE);

        let layer = layers.next().ok_or_else(|| {
            SourceError::Malformed(format!(
                "`{reference}` carries no `{LAYER_MEDIA_TYPE}` layer, so it is not a \
                 stanchion plugin"
            ))
        })?;

        // One directory, one layer. More than one means the artifact was built by
        // something with a different idea of this format, and guessing which layer is
        // the plugin is how a fetcher ends up unpacking the wrong thing.
        if layers.next().is_some() {
            return Err(SourceError::Malformed(format!(
                "`{reference}` carries more than one plugin layer"
            )));
        }

        Ok(layer.data.to_vec())
    }
}

/// Parses an `oci://` reference, or a bare registry reference.
fn parse_reference(reference: &str) -> Result<Reference, SourceError> {
    let bare = reference.strip_prefix(OCI_SCHEME).unwrap_or(reference);
    if bare.is_empty() {
        return Err(SourceError::Unsupported(reference.to_string()));
    }
    Reference::from_str(bare).map_err(|err| {
        SourceError::Unsupported(format!("`{reference}` is not an OCI reference: {err}"))
    })
}

/// Runs one async operation to completion from a synchronous caller.
///
/// Installation is a deliberate, blocking, machine-mutating step — the same shape as
/// `install_rocks` — so the public API here is synchronous. The runtime is built on a
/// dedicated thread rather than the caller's, so this is also safe to call from inside
/// a host's existing async runtime, where a nested `block_on` would otherwise panic.
fn block_on<Factory, Fut, T>(factory: Factory) -> Result<T, SourceError>
where
    Factory: FnOnce() -> Fut + Send,
    Fut: std::future::Future<Output = Result<T, oci_client::errors::OciDistributionError>>,
    T: Send,
{
    std::thread::scope(|scope| {
        let handle = scope.spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|err| {
                    SourceError::Transport(format!("starting a runtime for the fetch: {err}"))
                })?;
            runtime.block_on(factory()).map_err(translate)
        });
        handle
            .join()
            .map_err(|_| SourceError::Transport("the fetch thread panicked".to_string()))?
    })
}

/// Turns a registry error into one that says what a caller can do about it.
fn translate(err: oci_client::errors::OciDistributionError) -> SourceError {
    use oci_client::errors::OciDistributionError;
    match err {
        OciDistributionError::ImageManifestNotFoundError(reference)
        | OciDistributionError::ManifestParsingError(reference) => {
            SourceError::NotFound(reference)
        }
        OciDistributionError::AuthenticationFailure(message) => {
            SourceError::Transport(format!("authentication failed: {message}"))
        }
        other => SourceError::Transport(other.to_string()),
    }
}

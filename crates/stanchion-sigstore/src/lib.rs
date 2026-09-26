//! Sigstore-backed [`PluginVerifier`].
//!
//! Verification is keyless: the plugin author signs with an OIDC identity, Fulcio
//! issues a short-lived certificate, and the host checks the resulting bundle against
//! sigstore's trust root plus an identity policy it chooses. Authors hold no long-term
//! private key, and the host trusts an *identity* (`repo:acme/plugins`) rather than a
//! key fingerprint.
//!
//! # Producing a bundle
//!
//! The signed artifact is [`DirectoryDigest::preimage`], so plain `cosign` can produce
//! it — nothing here needs a bespoke signing tool:
//!
//! ```text
//! # write the canonical bytes for a plugin directory
//! mlua-plugin-digest plugins/weather > weather.canonical
//! cosign sign-blob --bundle plugins/weather/plugin.sigstore.json weather.canonical
//! ```
//!
//! # Cost
//!
//! The `sigstore-verify` feature pulls a large dependency graph — sigstore itself
//! brings TUF, HTTP and TLS stacks. It is optional for exactly that reason.

use std::fs;
use std::io;
use std::path::Path;

use sha2::{Digest as _, Sha256};
use sigstore::bundle::Bundle;
use sigstore::bundle::verify::VerificationPolicy;
use sigstore::bundle::verify::blocking::Verifier;

use stanchion_registry::signature::{
    BUNDLE_FILE, DirectoryDigest, PluginVerifier, Signer, VerifyError,
};

/// Verifies plugin signatures against sigstore.
pub struct SigstoreVerifier<P> {
    verifier: Verifier,
    policy: P,
    identity: String,
    offline: bool,
}

impl<P: VerificationPolicy> SigstoreVerifier<P> {
    /// Verifies against the public-good trust root.
    ///
    /// `identity` is what a successful verification reports as the signer. Sigstore's
    /// verification API answers *whether the bundle conforms to your policy*, not what
    /// the certificate subject was, so the reported identity is the one your policy
    /// enforced — keep the two in step.
    ///
    /// Fetching the trust root touches the network; construct the verifier once and
    /// reuse it.
    pub fn production(identity: impl Into<String>, policy: P) -> Result<Self, VerifyError> {
        let verifier = Verifier::production()
            .map_err(|err| VerifyError::Invalid(format!("trust root unavailable: {err}")))?;
        Ok(SigstoreVerifier {
            verifier,
            policy,
            identity: identity.into(),
            offline: true,
        })
    }

    /// Wraps a verifier the caller built, for a private Fulcio or Rekor deployment.
    pub fn with_verifier(identity: impl Into<String>, policy: P, verifier: Verifier) -> Self {
        SigstoreVerifier {
            verifier,
            policy,
            identity: identity.into(),
            offline: true,
        }
    }

    /// Whether to skip Rekor's online inclusion check.
    ///
    /// Defaults to `true`: loading plugins should not depend on reaching a log server.
    /// The bundle still carries its inclusion proof, which is checked either way.
    pub fn offline(mut self, offline: bool) -> Self {
        self.offline = offline;
        self
    }
}

impl<P: VerificationPolicy + Send + Sync> PluginVerifier
    for SigstoreVerifier<P>
{
    fn verify(&self, digest: &DirectoryDigest, dir: &Path) -> Result<Signer, VerifyError> {
        let path = dir.join(BUNDLE_FILE);
        let raw = match fs::read_to_string(&path) {
            Ok(raw) => raw,
            // An absent bundle is not a bad signature; let the registry decide.
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Err(VerifyError::Missing);
            }
            Err(err) => return Err(VerifyError::Io(err)),
        };

        let bundle: Bundle = serde_json::from_str(&raw)
            .map_err(|err| VerifyError::Invalid(format!("malformed {BUNDLE_FILE}: {err}")))?;

        // The bundle covers the canonical preimage, which is what the root digest is
        // taken over, so this checks every file in the plugin.
        let hasher = Sha256::new_with_prefix(digest.preimage());

        self.verifier
            .verify_digest(hasher, bundle, &self.policy, self.offline)
            .map_err(|err| VerifyError::Untrusted(err.to_string()))?;

        Ok(Signer::verified(
            self.identity.clone(),
            Some("sigstore".to_string()),
        ))
    }
}

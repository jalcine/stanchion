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

use stanchion_abi::signature::{
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

impl<P: VerificationPolicy + Send + Sync> PluginVerifier for SigstoreVerifier<P> {
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

        // #1 full extraction: verify identity before verification digest consumes bundle.
        let extracted = extract_cert_fields(&bundle);

        // The bundle covers the canonical preimage, which is what the root digest is
        // taken over, so this checks every file in the plugin.
        let hasher = Sha256::new_with_prefix(digest.preimage());

        self.verifier
            .verify_digest(hasher, bundle, &self.policy, self.offline)
            .map_err(|err| VerifyError::Untrusted(err.to_string()))?;

        // Compare extracted cert identity to constructor identity; fail loudly on mismatch.
        let issuer = match extracted {
            Some(fields) => match fields.identity {
                Some(id) if id == self.identity => fields.issuer,
                Some(id) => {
                    return Err(VerifyError::Untrusted(format!(
                        "sigstore identity mismatch: bundle cert says \"{id}\", policy/constructor expected \"{}\"",
                        self.identity
                    )));
                }
                None => {
                    return Err(VerifyError::Untrusted(
                        "sigstore bundle has no extractable certificate identity".to_string(),
                    ));
                }
            },
            None => {
                return Err(VerifyError::Untrusted(
                    "sigstore bundle has no extractable certificate identity".to_string(),
                ));
            }
        };

        // Report the OIDC issuer recorded in the Fulcio certificate, so a lockfile
        // `issuer` pin can actually bind it. When the certificate carries no issuer
        // extension the issuer is left unset rather than a misleading constant, so a
        // pin fails closed. See #38.
        Ok(Signer::verified(self.identity.clone(), issuer))
    }
}

/// The Fulcio OIDC issuer extension: v2 (DER-encoded UTF8String) and the legacy raw
/// form. See <https://github.com/sigstore/fulcio/blob/main/docs/oid-info.md>.
const OID_ISSUER_V2: &str = "1.3.6.1.4.1.57264.1.8";
const OID_ISSUER_LEGACY: &str = "1.3.6.1.4.1.57264.1.1";

/// Identity (SAN) and OIDC issuer pulled from a bundle's leaf certificate.
struct CertFields {
    identity: Option<String>,
    issuer: Option<String>,
}

/// Extract identity (SAN) and OIDC issuer from the bundle's leaf certificate.
fn extract_cert_fields(bundle: &Bundle) -> Option<CertFields> {
    let vm = bundle.verification_material.as_ref()?;
    let content = vm.content.as_ref()?;
    // Access the X509 certificate DER bytes from the bundle's verification material.
    // The `content` field is a oneof; we handle the two certificate variants.
    let der = match content {
        sigstore_protobuf_specs::dev::sigstore::bundle::v1::verification_material::Content::X509CertificateChain(chain) => chain.certificates.first()?,
        sigstore_protobuf_specs::dev::sigstore::bundle::v1::verification_material::Content::Certificate(cert) => cert,
        _ => return None,
    };
    let (_, parsed) = x509_parser::parse_x509_certificate(&der.raw_bytes).ok()?;
    Some(CertFields {
        identity: extract_identity(&parsed),
        issuer: extract_issuer(&parsed),
    })
}

/// The signer identity: the first URI SAN, or, failing that, the first email (RFC822)
/// SAN — email-identity certificates would otherwise always fail to match.
fn extract_identity(parsed: &x509_parser::certificate::X509Certificate) -> Option<String> {
    let san_ext = parsed.subject_alternative_name().ok()??;
    let mut email: Option<String> = None;
    for name in &san_ext.value.general_names {
        match name {
            x509_parser::extensions::GeneralName::URI(s) => return Some(s.to_string()),
            x509_parser::extensions::GeneralName::RFC822Name(s) if email.is_none() => {
                email = Some(s.to_string());
            }
            _ => {}
        }
    }
    email
}

/// The OIDC issuer recorded by Fulcio, preferring the v2 (DER-encoded) extension over
/// the legacy raw form.
fn extract_issuer(parsed: &x509_parser::certificate::X509Certificate) -> Option<String> {
    for oid in [OID_ISSUER_V2, OID_ISSUER_LEGACY] {
        if let Some(ext) = parsed
            .extensions()
            .iter()
            .find(|ext| ext.oid.to_id_string() == oid)
            && let Some(issuer) = decode_issuer_value(ext.value)
        {
            return Some(issuer);
        }
    }
    None
}

/// Decodes an issuer extension value: the v2 form is a DER UTF8String (tag `0x0c`); the
/// legacy form is the raw UTF-8 issuer string.
fn decode_issuer_value(value: &[u8]) -> Option<String> {
    // v2: DER UTF8String with a short-form length.
    if let [0x0c, len, rest @ ..] = value
        && *len < 0x80
        && let Some(body) = rest.get(..usize::from(*len))
        && let Ok(text) = std::str::from_utf8(body)
    {
        let text = text.trim();
        if !text.is_empty() {
            return Some(text.to_string());
        }
    }
    // Legacy (or anything not a clean DER UTF8String): treat as raw UTF-8.
    let text = std::str::from_utf8(value).ok()?.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_v2_der_utf8string_issuer() {
        // Fulcio's v2 extension wraps the issuer as a DER UTF8String (tag 0x0c).
        let issuer = "https://token.actions.githubusercontent.com";
        let mut der = vec![0x0c, issuer.len() as u8];
        der.extend_from_slice(issuer.as_bytes());
        assert_eq!(decode_issuer_value(&der).as_deref(), Some(issuer));
    }

    #[test]
    fn decodes_a_legacy_raw_issuer() {
        // The legacy extension stores the issuer as a raw UTF-8 string.
        let issuer = "https://accounts.google.com";
        assert_eq!(
            decode_issuer_value(issuer.as_bytes()).as_deref(),
            Some(issuer)
        );
    }

    #[test]
    fn a_raw_value_that_happens_to_start_like_der_still_round_trips() {
        // A raw issuer whose length byte does not describe the buffer falls back to raw.
        let issuer = "https://issuer.example";
        assert_eq!(
            decode_issuer_value(issuer.as_bytes()).as_deref(),
            Some(issuer)
        );
    }

    #[test]
    fn an_empty_issuer_value_is_none() {
        assert_eq!(decode_issuer_value(&[]), None);
        assert_eq!(decode_issuer_value(b"   "), None);
    }
}

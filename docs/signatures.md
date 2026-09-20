# Signatures

Binding an identity to the exact bytes that will run, and using that
provenance to tier [capabilities](capabilities.md).

# Signatures

A signature binds **an identity** to **the exact bytes that will run**, checked against
a trust root the host controls before any Lua executes.

## What is signed

Every file in the plugin directory, not just `plugin.toml`. Signing the manifest alone
would be worse than useless: swap `init.lua`, leave the manifest, and an audit reports
capabilities the code does not match. `DirectoryDigest` hashes each file and folds them
in sorted order:

```text
for each file, sorted by path (excluding the signature artifact):
    relative path ‖ 0x00 ‖ sha256(contents) ‖ 0x00
```

Those bytes — `DirectoryDigest::preimage` — are the signed artifact, so plain `cosign`
can produce a bundle without any bespoke tool. The per-file hashes are kept, and the
loader re-checks each file as it reads it, so what runs is what was verified even if
the directory changed in between.

## Sigstore

The `sigstore-verify` feature provides keyless verification: the author signs with an
OIDC identity, Fulcio issues a short-lived certificate, and the host checks the bundle
against sigstore's trust root plus an identity policy of its choosing. Authors hold no
long-term key, and the host trusts an *identity* rather than a key fingerprint.

```rust
use sigstore::bundle::verify::policy::GitHubWorkflowRepository;

let registry = Registry::isolated(Lua::new(), Sandbox::restricted())
    .with_verifier(SigstoreVerifier::production(
        "repo:acme/plugins",
        GitHubWorkflowRepository::new("acme/plugins"),
    )?)
    .require_signatures(true);
```

Verification is offline by default — loading plugins should not depend on reaching a
log server; the bundle's inclusion proof is checked either way.

Two costs worth knowing. `sigstore-verify` pulls roughly **246 transitive
dependencies**, including TUF, HTTP and TLS stacks, which is why it is optional. And
sigstore-rs describes itself as experimental with an API that can change — which is why
verification sits behind the `PluginVerifier` trait rather than being wired in
directly, so you can supply your own PKI instead.

## Unsigned plugins

`require_signatures(false)`, the default, loads an unsigned plugin as
`Signer::Unsigned`. That makes signing a gradient rather than a cliff — existing
plugins keep working, and policy decides what they may reach.

## Provenance tiers capabilities

This is where signing stops being a checkbox. `CapabilityRequest` carries the verified
signer, so the policy can grant by who signed:

```rust
Rules::deny_all().allow_with("network", |request| match request.signer().identity() {
    Some(id) if id.starts_with("repo:acme/") => Decision::Grant,
    _ => Decision::deny("network requires a first-party signature"),
})
```

Pair that with `optional = true` and an unsigned plugin degrades gracefully instead of
failing: it loads, just without the capability.

`audit` reports each plugin's signer alongside its requests, still without running any
plugin code.

## What signatures do not buy

- **Origin and integrity, not safety.** A verified plugin from a trusted author can
  still be hostile. A signature says whom to hold responsible.
- **Revocation is separate.** A withdrawn plugin's signature stays valid; that needs a
  denylist the host refreshes.

---

[← Documentation index](../README.md#documentation)

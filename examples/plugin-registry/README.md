# plugin-registry

A self-contained plugin registry: publish a plugin, serve it with a gated
upload endpoint, and install it into a host that pins what it runs.

```sh
# Terminal 1: publish the fixture, then serve the content directory.
cargo run -q -p plugin-registry --bin registry-publish -- \
  ./plugins/greeter /tmp/reg-content --source-base http://127.0.0.1:18080
printf 'repo:acme/plugins\n' > /tmp/trust-root
cargo run -q -p plugin-registry --bin registry-serve -- \
  /tmp/reg-content /tmp/trust-root 127.0.0.1:18080

# Terminal 2: install and run. Refuses without --yes (a first install widens).
cargo run -q -p plugin-registry --bin registry-consume -- \
  http://127.0.0.1:18080 /tmp/reg-host greeter '^1.0' --yes
```

## The trust story

Three parties, none of which trusts the others:

- **Publish** records a claim: name, version, digest, and the signer named by
  `plugin.sig`. An unsigned directory is refused — there is no claim to record.
- **Serve** holds no authority. `GET` serves catalog, releases (with a fresh
  expiry window), and packages. `POST /v1/upload` promotes only after five
  checks: shaped name/version/digest, safe unpack to staging, staged digest
  equals claimed digest, `plugin.sig` names exactly the claimed signer, and the
  signer is in the trust root. Anything else is quarantined with 422 and leaves
  nothing behind.
- **Consume** re-verifies everything: digest after fetch, manifest name and
  version, upgrade review before commit. A lockfile pin then overrides the index
  on every later run — try publishing 1.1.0 and re-running consume: the host
  stays on the pinned 1.0.0 until someone deliberately upgrades.

The signature check here is demo-grade on purpose: `plugin.sig` names an
identity and the trust root lists trusted identities, but nothing binds the
identity to the digest cryptographically. A production server would verify a
[Sigstore bundle](../../crates/stanchion-sigstore) against its trust root at the
same gate; the shape — verify before promote, quarantine on failure — is
unchanged.

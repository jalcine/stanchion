# Distribution

Getting a plugin from whoever wrote it onto the machine that runs it, over
infrastructure none of which has to be trusted.

[Signatures](signatures.md) answer one question: *who produced these bytes?* Shipping
plugins raises three more, and a signature answers none of them.

| Question | Answered by |
| --- | --- |
| *Which* build is the legitimate one? | the lockfile |
| Can the place it came from lie to me? | it can — the pin is checked after the fetch |
| Is this upgrade asking for more than the last one? | the upgrade review |

These are not hypotheticals. Substituting a different build for the name you asked
for, downgrading you to an older signed-but-vulnerable release, and publishing a new
version with one more line in `[capabilities]` are all attacks that work **with a
valid signature**, and the last two work with the author's real key and no compromise
of anything.

```text
  index (untrusted)          OCI registry (untrusted)
        │ name + requirement         │ tar.gz
        ▼                            ▼
  ┌────────────────────────────────────────────┐
  │ stage: unpack ‖ digest ‖ manifest ‖ review │   nothing installed yet
  └────────────────────────────────────────────┘
        │ a person, or a policy, accepts
        ▼
  lockfile pin  ──►  the registry refuses anything else, at every load
```

Every box the bytes pass through is untrusted except the lockfile, which the host
writes and commits to its own repository.

## The lockfile is the trust anchor

`stanchion.lock` states, per plugin, the exact bytes that may load:

```toml
version = 1

[plugins.formatter]
version = "1.4.2"
digest = "sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
signer = "repo:acme/plugins"
source = "oci://ghcr.io/acme/plugins/formatter:1.4.2"
```

```rust
let registry = Registry::isolated(Lua::new(), Sandbox::restricted())
    .with_lockfile(Lockfile::load("stanchion.lock")?);
```

`digest` is the [`DirectoryDigest`](signatures.md#what-is-signed) root — the same
number a signature covers, so pinning adds no second hashing scheme. Everything else
narrows further, and `source` carries no authority whatsoever: it says where bytes
came from, never whether to accept them.

Three properties follow, and they are the reason to do it this way:

- **Substitution fails.** A registry serving different bytes produces a different
  digest. Nothing else has to go right for this to hold.
- **Downgrades fail.** An explicit pin is not something a server can move.
- **`signer` means what people think verification means.** "Signed by someone the
  trust root accepts" is a far weaker claim than "signed by the author I chose".
  Pinning the identity is what closes that gap.

A plugin in the root with no entry fails as `Unlocked` rather than loading. Once a
host keeps a lockfile, an unpinned directory appearing beside the pinned ones is
exactly the event worth refusing.

Pinning works with **no signing infrastructure at all** — a digest is a digest.
Signatures add attributable authorship on top; they are not a prerequisite.

## Packages are OCI artifacts

```text
manifest  artifactType: application/vnd.stanchion.plugin.v1+json
  config  application/vnd.stanchion.plugin.config.v1+json    the manifest's metadata
  layer   application/vnd.stanchion.plugin.layer.v1.tar+gzip  the plugin directory
```

An OCI registry is a content-addressed blob store with authentication, mirroring,
retention and access control already solved, available from every cloud and runnable
locally in one command. Using one means a host running a plugin ecosystem does not
also have to run a package server.

One layer holds the plugin directory, including its `plugin.sigstore.json` — the
directory digest [excludes the signature artifact](signatures.md#what-is-signed) from
itself precisely so a package can carry its own signature.

**The registry's digest is not the pin.** OCI addresses the compressed tarball;
stanchion addresses the directory. Repacking changes the first and leaves the second
alone, which is what makes mirroring safe: a mirror can recompress and re-tag and
still cannot alter a file without breaking the signature. The flip side is that **tar
metadata is covered by nothing** — modes, owners, mtimes and entry types are
attacker-controlled even in a correctly signed package, so unpacking ignores all of
them.

## Unpacking refuses more than it accepts

An archive from a registry is hostile input *before* anything about it is verified:
the digest cannot be checked until the files are on disk. So extraction refuses

- absolute paths, `..` components, and paths with a root or drive prefix
- symlinks and hard links, whatever they point at
- anything that is not a regular file or a directory
- the same path appearing twice
- more than 4096 files, 64 MiB total, or 16 MiB in one file (`Limits`)

Symlinks are refused rather than sanitised, because a symlink cannot be represented in
a `DirectoryDigest` at all: the digest hashes what `std::fs::read` returns, and for a
symlink that is whatever it points at *on the machine doing the hashing*. A plugin
containing one would verify on the signer's machine and mean something else on yours.

## Running an index

An index is **two static JSON documents**. Anything that can serve a file can host
one — S3, GitHub Pages, nginx, a git repository people clone. There is no database and
no API, because an index holds no authority worth protecting.

```text
<base>/v1/index.json              the catalog: which plugins exist
<base>/v1/plugins/<name>.json     the releases of one plugin
```

```json
{
  "schema": 1,
  "name": "formatter",
  "updated": "2026-09-20T00:00:00Z",
  "expires": "2026-09-27T00:00:00Z",
  "releases": [
    {
      "version": "1.4.2",
      "digest": "sha256:9f86d081884c7d65…",
      "source": "oci://ghcr.io/acme/plugins/formatter:1.4.2",
      "signer": "repo:acme/plugins",
      "issuer": "https://token.actions.githubusercontent.com",
      "capabilities": ["network"],
      "yanked": false
    }
  ]
}
```

The catalog is the same shape with a `plugins` array of names, and is optional: an
index that cannot enumerate is still usable for everything else.

| Field | Weight |
| --- | --- |
| `digest` | the only field with any. It becomes the pin, and is checked against the bytes that arrive. |
| `version` | checked against the manifest inside the package; a disagreement fails the install. |
| `source` | a hint. Fetching the same digest from anywhere else is equally acceptable. |
| `signer`, `issuer` | copied into the pin, then enforced by the verifier at load. |
| `capabilities` | advisory, for browsing. The manifest is what the registry reads. |
| `yanked` | stops new pins. It does **not** stop an existing pin from loading. |

**`expires` is not decoration.** An attacker who can stop you reaching the real index
cannot forge a release, but they can serve you last month's snapshot forever — hiding
a yank, or hiding that a fixed version exists. An expiry bounds how long that works,
which is why `Freshness::Required` (the default over HTTP) refuses a document that has
no expiry at all. The cost is that an index must be re-published periodically; an
index nobody maintains stops being believed, which is the correct behaviour for a
stale mirror.

Releases are ordered by semver, not by document order, so an index cannot steer a
client by reordering its own file.

## Implementing an index

Serve those two paths, or implement the trait against whatever you already run:

```rust
pub trait PluginIndex {
    fn releases(&self, name: &str) -> Result<PluginReleases, IndexError>;
    fn catalog(&self) -> Result<Catalog, IndexError> { /* optional */ }
    fn resolve(&self, name: &str, req: &VersionReq) -> Result<Release, IndexError> { /* provided */ }
}
```

`DirectoryIndex` reads the layout from a directory and `HttpIndex` reads it over
HTTPS. The trait is deliberately thin because an index is not trusted: it maps a name
to candidate digests, and every claim it makes is checked against the package that
actually arrives.

The crate is dependency-light by default for this reason — an index *publisher* can
depend on `stanchion-dist` for the document types alone, without an HTTP or TLS stack:

| Feature | Adds |
| --- | --- |
| *(none)* | index documents, `PluginIndex`, `DirectoryIndex` |
| `package` | packing and unpacking packages, `Installer` |
| `oci` | `OciSource`: fetching and publishing to an OCI registry |
| `http` | `HttpIndex` |
| `client` | `oci` plus `http` |

TLS authenticates the *server*, not what it said. An index reached over a flawless TLS
connection to a compromised bucket is a compromised index; HTTPS here keeps a passive
network from seeing which plugins you run, and is not what stands between you and a
bad one.

## Publishing

```rust
let mut archive = Vec::new();
let digest = package::pack(Path::new("plugins/formatter"), &mut archive)?;

OciSource::authenticated(auth).publish(
    "oci://ghcr.io/acme/plugins/formatter:1.4.2",
    archive,
    &ArtifactConfig { /* name, version, digest, capabilities */ },
)?;
```

Then add a release entry naming that digest and re-publish the index document. Sign
the directory [as before](signatures.md) — `cosign sign-blob` over the canonical
preimage — and include the bundle in the directory so the package carries it.

Packing normalises entry metadata (fixed mtime, mode, ownership) and writes entries in
the digest's own sorted order, so packing one directory twice produces identical bytes.
Nothing depends on that, since the pin is over the directory, but two packages that
disagree are much easier to reason about when repacking is deterministic.

## Installing is staged, and the gap in the middle is the point

```rust
let staged = installer.stage_upgrade("formatter", &"^1.0".parse()?, &lockfile)?;

if let Some(review) = staged.review() {
    if review.widens() {
        for concern in review.concerns() { eprintln!("  {concern}"); }
        staged.discard()?;
        return Ok(());
    }
}

lockfile.pin("formatter", staged.commit()?);
lockfile.save("stanchion.lock")?;
```

Fetching and unpacking happen in a staging directory beside the plugin root. `commit`
is a rename; `discard` is a delete. A plugin that fails any check never exists in the
root — not briefly, not half-written — so the registry is never asked to reason about
a directory that is mid-install. A previous version is moved aside and deleted only
once the new one is in place, so an interrupted commit leaves either the old plugin or
the new one.

### The upgrade review

`audit` reads manifests without running any plugin code. Pointed at two versions of
one plugin, the same idea answers the question that actually matters when bytes arrive
from elsewhere:

```text
formatter 1.4.2 -> 1.5.0
  + capability `network`
  ! capability `fs`: { paths = ["./data"] } -> { paths = ["./data", "/etc"] }
  + rock `luasocket` >= 3.0 (unsigned, outside the sandbox)
```

Stealing a signing key is hard. Publishing version 1.5.0 of a plugin people already
trust, with one more capability, is not — and a signature does not help, because the
malicious version is correctly signed by the author whose account was taken. `widens()`
is the predicate to gate on: a review that does not widen can be applied unattended,
and one that does needs a person. A first install always counts as widening, because
there is nothing to compare against.

Narrowing is inferred conservatively. Capability parameters are host-defined, so this
cannot know that `hosts = ["*.acme.com"]` is wider than `["api.acme.com"]`; anything
it cannot *prove* is a narrowing counts as a widening. It errs towards noisy.

## Yanking is not revocation

| | Stops new installs | Stops a pinned build from loading |
| --- | --- | --- |
| `yanked` in the index | yes | **no** |
| [`Revocations`](signatures.md#revocation) | yes | yes, at next load |

A yank is advice from a publisher; a revocation is a decision by the host. If you need
a build to stop running on machines that already have it, you need the revocation
list, and it is consulted at load rather than at install for exactly that reason.

## What this does not buy

- **It does not make a plugin safe.** It makes the bytes you run the bytes you chose.
  Whether that choice was good is what [capabilities](capabilities.md) and the
  [sandbox](isolation.md) are for.
- **Availability is not integrity.** An index that stops answering, or answers only
  about old releases, cannot be caught by a digest check. Expiry bounds how long a
  withheld update goes unnoticed; nothing makes an unreachable source tell the truth.
- **First contact is a decision, not a verification.** Pinning a plugin you have never
  seen records whatever the index said. The signer pin and the capability review are
  what make writing that entry a judgement rather than a transcription.
- **`[rocks]` are a second supply chain.** Nothing here signs, pins or unpacks a
  LuaRocks package, and a [C rock](luarocks.md#c-modules) is native code outside the
  sandbox entirely. A newly declared rock is reported as a widening because that is all
  this layer can do about it.

---

[← Documentation index](../README.md#documentation)

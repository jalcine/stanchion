# plugin-index

A static plugin index on disk: the two JSON documents a host resolves against.

```text
index/
  v1/index.json              the catalog: which plugins exist
  v1/plugins/<name>.json     the releases of one plugin
```

Anything that serves files can host these — S3, GitHub Pages, nginx, or a
checkout someone clones. There is no database and no API, because an index
holds no authority worth protecting: it maps a name to candidate digests, and
every claim it makes is checked against the package that actually arrives.

## Using it

Point a `DirectoryIndex` at `index/` (freshness is `Lenient` there: a
directory you generated yourself has no separate party who could withhold an
update). Resolving a requirement yields a release; fetching its `source`,
unpacking, and verifying the directory digest is what
[`Installer::stage_release`](../../../crates/stanchion-dist/src/install.rs)
does. What gets pinned in `stanchion.lock` and how upgrades are reviewed is
what the [`pinned`](../../../crates/stanchion/examples/pinned.rs) example
runs end to end.

```rust
let index = DirectoryIndex::new("examples/plugin-index/index");
let release = index.resolve("formatter", &"^1.0".parse()?)?;
// `release.digest` becomes the pin; `release.source` is only a hint.
```

## Digests

The `digest` fields below are placeholders — `sha256:<hex of pack output>`.
Publish for real by packing the plugin directory and pasting its root digest:

```rust
let mut archive = Vec::new();
let digest = stanchion_dist::package::pack("plugins/formatter", &mut archive)?;
// name `digest.hex()` in the release document, put the archive somewhere
// fetchable, name that URL in `source`, and re-publish before `expires`.
```

`expires` bounds how long a withheld snapshot stays believable. An index
nobody re-publishes stops being believed, which is the correct behaviour for
a stale mirror.

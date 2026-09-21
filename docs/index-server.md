# Serving an index

An [index](distribution.md#running-an-index) is two JSON documents and a pile of
immutable package files, so anything that serves bytes can host one. A *server* earns
its place by doing what static hosting cannot.

| It buys you | Why static hosting cannot |
| --- | --- |
| **A fresh `expires` on every response** | a client's default refuses a document without an unexpired one, so static hosting means re-publishing on a schedule |
| **A source that is not a directory** | a database, a monorepo, whatever answers `IndexSource` |
| **Conditional requests** | a client that already has the current document pays for headers, not a body |

If none of those apply, `DirectoryIndex` plus `aws s3 sync` is the better answer.

## Routes

```text
GET|HEAD  /v1/index.json              the catalog
GET|HEAD  /v1/plugins/<name>.json     one plugin's releases
GET|HEAD  /v1/blobs/sha256:<hex>      the package for that digest
```

Paths come from `CATALOG_PATH` and `releases_path` in `stanchion-dist`, so the server
and the client cannot drift apart when the schema moves to v2.

**404 is load-bearing.** A client maps 404 to "no such plugin" and every other non-2xx
to "the index is unreachable" — the difference between reporting a missing plugin and
retrying. An unknown plugin or digest is 404; a malformed name or digest is 400.

## Freshness, and why time is quantised

`expires` is computed per response from a TTL — but from the **start of the TTL
window**, not from the wall clock:

```rust
IndexServer::new(DirectorySource::new("/srv/plugins"))
    .ttl(Duration::from_secs(3600))
```

That detail is what makes conditional requests possible at all. A document that took
`expires` from `now` would have different bytes on every request, so an `ETag` over
the body could never match and `If-None-Match` would be dead weight. Quantised, every
response inside a window is byte-identical, the `ETag` is a plain content hash, and
`Cache-Control: max-age` counts down to the window's end.

It also closes the obvious hole: a `304` lets a client keep the body it has, *including
the `expires` it was served with*. Because a match can only occur inside the window
that issued it, a conditional request can never extend a document past its expiry — at
the boundary the bytes change, the validator stops matching, and the client is handed a
fresh document.

The TTL is the one real tuning decision, so it is clamped to 60s–24h. Too short and
clients with skewed clocks start refusing documents; too long and a yank takes that
long to reach someone being fed a withheld snapshot.

Packages are immutable — a package is named by the digest of what it unpacks to — so
they are served with `immutable` and their digest as the `ETag`.

## Framework independence

Nothing in `stanchion-index` knows about a web framework. `IndexServer::serve` takes a
method, a path and an optional `If-None-Match`, and returns a `Served`: a status,
headers and bytes. Routing, freshness and conditional requests are all tested without
binding a port.

| Adapter | For |
| --- | --- |
| [`stanchion-index-poem`](../crates/stanchion-index-poem) | [Poem](https://docs.rs/poem), whose `Endpoint` is not a `tower::Service` |
| the `tower` feature of `stanchion-index` | anything that is one — axum, tonic, warp |

Poem needs its own adapter because it is **not** Tower-based: it has its own `Endpoint`
trait, and its `tower-compat` feature only lets it *consume* `tower::Layer`, pinned to
tower 0.4 while axum is on 0.5. Any other framework is a ten-line match on `Served`.

The source is synchronous, matching `PluginIndex`, so adapters run it on a blocking
worker rather than an async one.

## On-disk layout

```text
<base>/v1/index.json              the catalog, optional
<base>/v1/plugins/<name>.json     one plugin's releases
<base>/blobs/sha256/<hex>         the package for that digest
```

Packages live under a `sha256/` segment rather than a `sha256:<hex>` filename, because
a colon in a filename is legal on Linux and a nuisance everywhere else.

Stored documents are read with `Freshness::Ignored`: their own `expires` is irrelevant
when the server issues a new one. A directory too stale for a client to accept is still
a perfectly good source for a server that keeps it fresh.

## What this does not do

- **No write path.** Publishing is putting an archive somewhere fetchable and adding a
  release to the index — two file writes. A publish API would make the server
  authoritative over what gets pinned, which is a different trust model.
- **No authority.** Everything served is re-checked against a digest the client holds.
  A compromised bucket reached over flawless TLS is a compromised index.
- **No streaming.** A package is read into memory before it is served, which suits
  plugin-sized archives and would need revisiting for large ones.

---

[← Documentation index](../README.md#documentation)

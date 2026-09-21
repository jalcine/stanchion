//! Serve a stanchion plugin index over HTTP, independent of any web framework.
//!
//! An [index](stanchion_dist::index) is two JSON documents and a pile of immutable
//! package files, so anything that serves bytes can host one. A *server* earns its
//! place by doing the things static hosting cannot:
//!
//! - **Fresh expiry.** A client's default is
//!   [`Freshness::Required`](stanchion_dist::Freshness), which refuses a document
//!   carrying no unexpired `expires`. Static hosting means re-publishing on a
//!   schedule; this computes the expiry per response.
//! - **A source that is not a directory** — a database, a monorepo, whatever answers
//!   [`IndexSource`].
//! - **Conditional requests**, so a client that already has the current document pays
//!   for a header exchange rather than a body.
//!
//! # Framework independence
//!
//! Nothing here knows about a web framework. [`IndexServer::serve`] takes a method, a
//! path and an optional `If-None-Match`, and returns a [`Served`] — a status, headers
//! and bytes. Adapters translate that one type:
//!
//! - `stanchion-index-poem` for [Poem](https://docs.rs/poem), whose `Endpoint` is not
//!   a `tower::Service`.
//! - the `tower` feature here for everything that is one: axum, tonic, warp.
//!
//! Any other framework is a ten-line match on [`Served`].
//!
//! ```ignore
//! let server = IndexServer::new(DirectorySource::new("/srv/plugins"))
//!     .ttl(Duration::from_secs(3600));
//!
//! let served = server.serve(&Method::GET, "/v1/index.json", None);
//! ```

mod server;
mod source;

#[cfg(feature = "stream")]
mod stream;

#[cfg(feature = "tower")]
mod tower;

pub use server::{Body, IndexServer, Served, DEFAULT_TTL};
pub use source::{Blob, DirectorySource, IndexSource, BLOBS_PREFIX, BLOBS_SUBDIR};

#[cfg(feature = "stream")]
pub use stream::{chunks, chunks_of, DEFAULT_CHUNK};

#[cfg(feature = "tower")]
pub use tower::IndexService;

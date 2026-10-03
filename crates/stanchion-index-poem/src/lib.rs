//! [Poem](https://docs.rs/poem) adapter for a stanchion plugin index.
//!
//! Poem has its own `Endpoint` trait rather than being built on Tower, so it needs an
//! adapter of its own. All this does is translate one type: a
//! [`stanchion_index::Served`] into a `poem::Response`. Everything worth
//! testing — routing, freshness, conditional requests — lives in
//! [`stanchion_index`] and is tested without binding a port.
//!
//! ```ignore
//! use poem::{listener::TcpListener, Server};
//! use stanchion_index::{DirectorySource, IndexServer};
//! use stanchion_index_poem::index_endpoint;
//!
//! let server = IndexServer::new(DirectorySource::new("/srv/plugins"));
//! Server::new(TcpListener::bind("0.0.0.0:8080"))
//!     .run(index_endpoint(server))
//!     .await?;
//! ```

use std::sync::Arc;

use poem::http::StatusCode;
use poem::{Endpoint, Request, Response};
use stanchion_index::{Body, IndexServer, IndexSource, Served, request_parts};

/// Wraps an [`IndexServer`] as a Poem endpoint.
///
/// Mount it at the root, or under a prefix with `poem::Route::nest` — the server reads
/// the path it is given and answers 404 for anything outside its own routes.
pub fn index_endpoint<S>(server: IndexServer<S>) -> IndexEndpoint<S>
where
    S: IndexSource + Send + Sync + 'static,
{
    IndexEndpoint {
        server: Arc::new(server),
    }
}

/// A Poem endpoint serving a plugin index.
pub struct IndexEndpoint<S> {
    server: Arc<IndexServer<S>>,
}

impl<S> Clone for IndexEndpoint<S> {
    fn clone(&self) -> Self {
        IndexEndpoint {
            server: Arc::clone(&self.server),
        }
    }
}

impl<S> Endpoint for IndexEndpoint<S>
where
    S: IndexSource + Send + Sync + 'static,
{
    type Output = Response;

    async fn call(&self, request: Request) -> poem::Result<Self::Output> {
        let (method, path, if_none_match) = request_parts(
            request.method(),
            request.uri().path(),
            request.headers(),
        );

        let server = Arc::clone(&self.server);
        // The source is synchronous — a directory read, or whatever the host wrote —
        // so it runs off the async worker rather than blocking it.
        let served = tokio::task::spawn_blocking(move || {
            server.serve(&method, &path, if_none_match.as_deref())
        })
        .await
        .map_err(|err| {
            poem::Error::from_string(err.to_string(), StatusCode::INTERNAL_SERVER_ERROR)
        })?;

        Ok(into_response(served))
    }
}

/// Translates a [`Served`] into a Poem response.
///
/// A document becomes bytes; a package becomes a stream, so serving a large one costs
/// a buffer rather than its own size in memory. Which headers an answer carries is
/// [`stanchion_index::Served::headers`]' business — only the body is Poem-specific.
pub fn into_response(served: Served) -> Response {
    let mut builder = Response::builder().status(served.status);
    for (name, value) in served.headers() {
        builder = builder.header(name, value);
    }

    match served.body {
        Body::Bytes(bytes) => builder.body(bytes),
        // Read on a blocking worker, delivered through a bounded channel.
        Body::Reader(reader) => builder.body(poem::Body::from_bytes_stream(
            stanchion_index::chunks(reader),
        )),
    }
}

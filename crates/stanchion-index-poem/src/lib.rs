//! [Poem](https://docs.rs/poem) adapter for a stanchion plugin index.
//!
//! Poem has its own `Endpoint` trait rather than being built on Tower, so it needs an
//! adapter of its own. All this does is translate one type: a
//! [`Served`](stanchion_index::Served) into a `poem::Response`. Everything worth
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

use poem::http::{header, HeaderValue, StatusCode};
use poem::{Endpoint, Request, Response};
use stanchion_index::{IndexServer, IndexSource, Served};

/// Wraps an [`IndexServer`] as a Poem endpoint.
///
/// Mount it at the root, or under a prefix with `poem::Route::nest` — the server reads
/// the path it is given and answers 404 for anything outside its own routes.
pub fn index_endpoint<S>(server: IndexServer<S>) -> IndexEndpoint<S>
where
    S: IndexSource + Send + Sync + 'static,
{
    IndexEndpoint { server: Arc::new(server) }
}

/// A Poem endpoint serving a plugin index.
pub struct IndexEndpoint<S> {
    server: Arc<IndexServer<S>>,
}

impl<S> Clone for IndexEndpoint<S> {
    fn clone(&self) -> Self {
        IndexEndpoint { server: Arc::clone(&self.server) }
    }
}

impl<S> Endpoint for IndexEndpoint<S>
where
    S: IndexSource + Send + Sync + 'static,
{
    type Output = Response;

    async fn call(&self, request: Request) -> poem::Result<Self::Output> {
        let method = request.method().clone();
        let path = request.uri().path().to_string();
        let if_none_match = request
            .headers()
            .get(header::IF_NONE_MATCH)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);

        let server = Arc::clone(&self.server);
        // The source is synchronous — a directory read, or whatever the host wrote —
        // so it runs off the async worker rather than blocking it.
        let served = tokio::task::spawn_blocking(move || {
            server.serve(&method, &path, if_none_match.as_deref())
        })
        .await
        .map_err(|err| poem::Error::from_string(err.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;

        Ok(into_response(served))
    }
}

/// Translates a [`Served`] into a Poem response.
pub fn into_response(served: Served) -> Response {
    let mut builder = Response::builder()
        .status(served.status)
        .header(header::CONTENT_TYPE, served.content_type);

    if let Ok(value) = HeaderValue::from_str(&served.cache_control) {
        builder = builder.header(header::CACHE_CONTROL, value);
    }
    if let Some(etag) = &served.etag
        && let Ok(value) = HeaderValue::from_str(etag)
    {
        builder = builder.header(header::ETAG, value);
    }
    builder.body(served.body)
}

//! A [`tower::Service`], so anything built on Tower can mount the index.
//!
//! axum, tonic and warp all take a `Service`; Poem does not, which is why it has an
//! adapter of its own. Both translate the same [`Served`](crate::Served).

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures_util::StreamExt;
use http::{header, HeaderValue, Request, Response, StatusCode};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full, StreamBody};
use tower_service::Service;

use crate::{Body, IndexServer, IndexSource, Served};

/// The body a mounted index answers with: bytes for documents, a stream for packages.
pub type IndexBody = BoxBody<Bytes, std::io::Error>;

/// Serves a plugin index as a Tower service.
pub struct IndexService<S> {
    server: Arc<IndexServer<S>>,
}

impl<S> IndexService<S> {
    /// Wraps an [`IndexServer`].
    pub fn new(server: IndexServer<S>) -> Self {
        IndexService { server: Arc::new(server) }
    }
}

impl<S> Clone for IndexService<S> {
    fn clone(&self) -> Self {
        IndexService { server: Arc::clone(&self.server) }
    }
}

impl<S, B> Service<Request<B>> for IndexService<S>
where
    S: IndexSource + Send + Sync + 'static,
{
    type Response = Response<IndexBody>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Infallible>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request<B>) -> Self::Future {
        let server = Arc::clone(&self.server);
        let method = request.method().clone();
        let path = request.uri().path().to_string();
        let if_none_match = request
            .headers()
            .get(header::IF_NONE_MATCH)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);

        Box::pin(async move {
            let served = server.serve(&method, &path, if_none_match.as_deref());
            Ok(into_response(served))
        })
    }
}

/// Translates a [`Served`] into an `http::Response`.
pub fn into_response(served: Served) -> Response<IndexBody> {
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
    if let Some(length) = served.content_length {
        builder = builder.header(header::CONTENT_LENGTH, length);
    }

    let body = match served.body {
        Body::Bytes(bytes) => Full::new(Bytes::from(bytes)).map_err(io_never).boxed(),
        // Read on a blocking worker, delivered through a bounded channel.
        Body::Reader(reader) => StreamBody::new(
            crate::stream::chunks(reader).map(|chunk| chunk.map(http_body::Frame::data)),
        )
        .boxed(),
    };

    builder.body(body).unwrap_or_else(|_| {
        let mut fallback = Response::new(Full::new(Bytes::new()).map_err(io_never).boxed());
        *fallback.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
        fallback
    })
}

/// `Full` cannot fail, so its error type is uninhabited and this never runs.
fn io_never(never: std::convert::Infallible) -> std::io::Error {
    match never {}
}

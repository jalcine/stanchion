//! A [`tower::Service`], so anything built on Tower can mount the index.
//!
//! axum, tonic and warp all take a `Service`; Poem does not, which is why it has an
//! adapter of its own. Both translate the same [`Served`](crate::Served).

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use http::{header, HeaderValue, Request, Response, StatusCode};
use tower_service::Service;

use crate::{IndexServer, IndexSource, Served};

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
    type Response = Response<Vec<u8>>;
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
pub fn into_response(served: Served) -> Response<Vec<u8>> {
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
    builder.body(served.body).unwrap_or_else(|_| {
        let mut fallback = Response::new(Vec::new());
        *fallback.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
        fallback
    })
}

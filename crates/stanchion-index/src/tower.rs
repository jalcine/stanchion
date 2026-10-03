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
use http::{Request, Response, StatusCode};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full, StreamBody};
use tower_service::Service;

use crate::{Body, IndexServer, IndexSource, Served, request_parts};

/// The body a mounted index answers with: bytes for documents, a stream for packages.
pub type IndexBody = BoxBody<Bytes, std::io::Error>;

/// Serves a plugin index as a Tower service.
pub struct IndexService<S> {
    server: Arc<IndexServer<S>>,
}

impl<S> IndexService<S> {
    /// Wraps an [`IndexServer`].
    pub fn new(server: IndexServer<S>) -> Self {
        IndexService {
            server: Arc::new(server),
        }
    }
}

impl<S> Clone for IndexService<S> {
    fn clone(&self) -> Self {
        IndexService {
            server: Arc::clone(&self.server),
        }
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
        let (method, path, if_none_match) = request_parts(
            request.method(),
            request.uri().path(),
            request.headers(),
        );

        Box::pin(async move {
            let served = server.serve(&method, &path, if_none_match.as_deref());
            Ok(into_response(served))
        })
    }
}

/// Translates a [`Served`] into an `http::Response`.
///
/// Which headers an answer carries is [`Served::headers`]' business, not this
/// function's: only the body is Tower-specific.
pub fn into_response(served: Served) -> Response<IndexBody> {
    let mut builder = Response::builder().status(served.status);
    for (name, value) in served.headers() {
        builder = builder.header(name, value);
    }

    // `BodyExt::boxed` by name, not by method call: `futures_util::StreamExt` is in
    // scope here for `.map`, it also has a `boxed`, and `StreamBody` satisfies both
    // traits. Which one `.boxed()` resolves to then depends on whether something else
    // in the build unified `futures-util`'s default features in — so the same source
    // compiles alone and is ambiguous beside another crate that pulls them.
    let body = match served.body {
        Body::Bytes(bytes) => BodyExt::boxed(Full::new(Bytes::from(bytes)).map_err(io_never)),
        // Read on a blocking worker, delivered through a bounded channel.
        Body::Reader(reader) => BodyExt::boxed(StreamBody::new(
            crate::stream::chunks(reader).map(|chunk| chunk.map(http_body::Frame::data)),
        )),
    };

    builder.body(body).unwrap_or_else(|_| {
        let mut fallback = Response::new(BodyExt::boxed(
            Full::new(Bytes::new()).map_err(io_never),
        ));
        *fallback.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
        fallback
    })
}

/// `Full` cannot fail, so its error type is uninhabited and this never runs.
fn io_never(never: std::convert::Infallible) -> std::io::Error {
    match never {}
}

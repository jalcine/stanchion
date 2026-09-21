//! Routing, freshness and conditional requests, with no framework in sight.

use std::time::Duration;

use http::{Method, StatusCode};
use sha2::{Digest, Sha256};
use stanchion_dist::{
    validate_name, IndexError, CATALOG_PATH, INDEX_SCHEMA,
};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::source::{IndexSource, BLOBS_PREFIX};

/// How long a served document stays believable, unless configured otherwise.
pub const DEFAULT_TTL: Duration = Duration::from_secs(3600);

/// A response, ready for any framework to translate.
#[derive(Debug, Clone)]
pub struct Served {
    /// Status to answer with.
    pub status: StatusCode,
    /// `Content-Type` of the body.
    pub content_type: &'static str,
    /// `Cache-Control`, already agreeing with the document's `expires`.
    pub cache_control: String,
    /// Strong `ETag`, quoted, or `None` where one is meaningless.
    pub etag: Option<String>,
    /// The body. Empty for `304` and for `HEAD`.
    pub body: Vec<u8>,
}

impl Served {
    fn json(status: StatusCode, body: Vec<u8>) -> Self {
        Served {
            status,
            content_type: "application/json",
            cache_control: "no-store".to_string(),
            etag: None,
            body,
        }
    }

    /// An error body in the index's own shape, so a client sees one format throughout.
    fn error(status: StatusCode, message: &str) -> Self {
        let body = serde_json::json!({ "error": message });
        Served::json(status, serde_json::to_vec(&body).unwrap_or_default())
    }
}

/// Serves an index from any [`IndexSource`].
pub struct IndexServer<S> {
    source: S,
    ttl: Duration,
}

impl<S: IndexSource> IndexServer<S> {
    /// Serves `source` with [`DEFAULT_TTL`].
    pub fn new(source: S) -> Self {
        IndexServer { source, ttl: DEFAULT_TTL }
    }

    /// Sets how long a served document stays believable.
    ///
    /// This is the one real tuning decision. Too long and a yank takes that long to
    /// reach someone being fed a withheld snapshot; too short and clients with skewed
    /// clocks start refusing documents, and caching stops helping. A zero or absurd
    /// TTL is clamped to something a client can actually use.
    pub fn ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl.clamp(Duration::from_secs(60), Duration::from_secs(86_400));
        self
    }

    /// The configured TTL.
    pub fn configured_ttl(&self) -> Duration {
        self.ttl
    }

    /// The source being served.
    pub fn source(&self) -> &S {
        &self.source
    }

    /// Answers one request.
    ///
    /// `path` may or may not carry a leading slash; anything outside the index's own
    /// routes is a 404 so a server can mount this beside other things.
    pub fn serve(&self, method: &Method, path: &str, if_none_match: Option<&str>) -> Served {
        self.serve_at(method, path, if_none_match, OffsetDateTime::now_utc())
    }

    /// Answers one request against a caller-supplied clock.
    ///
    /// Separate so expiry and conditional requests can be tested across a TTL
    /// boundary without waiting for one.
    pub fn serve_at(
        &self,
        method: &Method,
        path: &str,
        if_none_match: Option<&str>,
        now: OffsetDateTime,
    ) -> Served {
        if !(method == Method::GET || method == Method::HEAD) {
            return Served::error(StatusCode::METHOD_NOT_ALLOWED, "only GET and HEAD");
        }

        let route = path.trim_start_matches('/');
        let served = if route == CATALOG_PATH {
            self.catalog(now)
        } else if let Some(digest) = route.strip_prefix(BLOBS_PREFIX) {
            self.blob(digest)
        } else if let Some(name) = plugin_of(route) {
            self.releases(name, now)
        } else {
            Served::error(StatusCode::NOT_FOUND, "no such index route")
        };

        let served = apply_conditional(served, if_none_match);
        if method == Method::HEAD {
            return Served { body: Vec::new(), ..served };
        }
        served
    }

    fn catalog(&self, now: OffsetDateTime) -> Served {
        match self.source.catalog() {
            Ok(mut catalog) => {
                let (updated, expires) = self.window(now);
                catalog.schema = INDEX_SCHEMA;
                catalog.updated = Some(updated);
                catalog.expires = Some(expires);
                self.document(&catalog, now)
            }
            Err(err) => error_for(&err),
        }
    }

    fn releases(&self, name: &str, now: OffsetDateTime) -> Served {
        // The name came off a URL, so it is checked before it reaches a source.
        let Ok(name) = validate_name(name) else {
            return Served::error(StatusCode::BAD_REQUEST, "not a usable plugin name");
        };

        match self.source.releases(name) {
            Ok(mut releases) => {
                let (updated, expires) = self.window(now);
                releases.schema = INDEX_SCHEMA;
                releases.updated = Some(updated);
                releases.expires = Some(expires);
                self.document(&releases, now)
            }
            Err(err) => error_for(&err),
        }
    }

    fn blob(&self, digest: &str) -> Served {
        match self.source.blob(digest) {
            Ok(body) => Served {
                status: StatusCode::OK,
                content_type: stanchion_dist::PACKAGE_MEDIA_TYPE,
                // A package is named by the digest of what it unpacks to, so its bytes
                // can never change under that name.
                cache_control: "public, max-age=31536000, immutable".to_string(),
                etag: Some(format!("\"{digest}\"")),
                body,
            },
            Err(err) => error_for(&err),
        }
    }

    /// Serialises a document and gives it an `ETag` and a matching `Cache-Control`.
    fn document<T: serde::Serialize>(&self, document: &T, now: OffsetDateTime) -> Served {
        let Ok(body) = serde_json::to_vec_pretty(document) else {
            return Served::error(StatusCode::INTERNAL_SERVER_ERROR, "could not render");
        };

        Served {
            status: StatusCode::OK,
            content_type: "application/json",
            cache_control: format!("public, max-age={}", self.remaining(now)),
            etag: Some(etag_of(&body)),
            body,
        }
    }

    /// The TTL window containing `now`, as RFC 3339 `updated` and `expires`.
    ///
    /// Time is quantised to the TTL rather than taken from the clock, so every
    /// response inside a window is byte-identical. That is what makes an `ETag` over
    /// the body meaningful: without it a regenerated `expires` would change the body
    /// every second and no conditional request could ever match. It also guarantees a
    /// client refetches at least once per window rather than riding a 304 past expiry.
    fn window(&self, now: OffsetDateTime) -> (String, String) {
        let start = self.window_start(now);
        let expires = start.saturating_add(
            time::Duration::try_from(self.ttl).unwrap_or(time::Duration::HOUR),
        );
        (
            start.format(&Rfc3339).unwrap_or_default(),
            expires.format(&Rfc3339).unwrap_or_default(),
        )
    }

    fn window_start(&self, now: OffsetDateTime) -> OffsetDateTime {
        let ttl = self.ttl.as_secs().max(1) as i64;
        let unix = now.unix_timestamp();
        let start = unix.div_euclid(ttl).saturating_mul(ttl);
        OffsetDateTime::from_unix_timestamp(start).unwrap_or(now)
    }

    /// Seconds until the current window ends, for `max-age`.
    fn remaining(&self, now: OffsetDateTime) -> u64 {
        let ttl = self.ttl.as_secs().max(1) as i64;
        let elapsed = now.unix_timestamp().rem_euclid(ttl);
        u64::try_from(ttl.saturating_sub(elapsed)).unwrap_or(0)
    }
}

/// `v1/plugins/<name>.json` to `<name>`, derived from the same shape the client uses.
fn plugin_of(route: &str) -> Option<&str> {
    let prefix = stanchion_dist::releases_path("");
    let prefix = prefix.strip_suffix(".json")?;
    route.strip_prefix(prefix)?.strip_suffix(".json")
}

/// A strong, quoted `ETag` over the exact bytes served.
fn etag_of(body: &[u8]) -> String {
    let digest = Sha256::digest(body);
    let mut hex = String::with_capacity(16);
    for byte in digest.iter().take(8) {
        hex.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        hex.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0'));
    }
    format!("\"{hex}\"")
}

/// Turns a matching `If-None-Match` into a `304`.
///
/// A `304` means the client keeps the body it already has — whose `expires` is the one
/// it was served with. Because documents are quantised to TTL windows, a match can
/// only happen inside the window that produced it, so a conditional request can never
/// extend a document past its expiry.
fn apply_conditional(served: Served, if_none_match: Option<&str>) -> Served {
    let (Some(etag), Some(candidates)) = (&served.etag, if_none_match) else {
        return served;
    };
    if !served.status.is_success() {
        return served;
    }

    let matched = candidates == "*"
        || candidates
            .split(',')
            .map(str::trim)
            .any(|candidate| candidate.trim_start_matches("W/") == etag);

    if matched {
        Served { status: StatusCode::NOT_MODIFIED, body: Vec::new(), ..served }
    } else {
        served
    }
}

/// Maps a source's failure to a status.
///
/// `UnknownPlugin` must be a 404 and nothing else: a client maps 404 to "no such
/// plugin" and every other status to "the index is unreachable", which is the
/// difference between giving up and retrying.
fn error_for(err: &IndexError) -> Served {
    let status = match err {
        IndexError::UnknownPlugin(_) => StatusCode::NOT_FOUND,
        IndexError::Malformed(_) => StatusCode::BAD_REQUEST,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    Served::error(status, &err.to_string())
}

//! Serves a content directory as a plugin index, with a gated upload endpoint.
//!
//! ```sh
//! registry-serve ./content ./trust-root 127.0.0.1:8080
//! ```
//!
//! `GET` and `HEAD` are answered by the index server: catalog, release documents
//! with a fresh expiry window, and packages streamed off disk. `POST /v1/upload`
//! is the only write, and it promotes nothing on faith:
//!
//! ```text
//! POST /v1/upload?name=<name>&version=<version>&signer=<id>&digest=<hex>
//! Content-Type: application/gzip
//! <tar.gz>
//! ```
//!
//! 1. The name, version and digest are shaped before anything is read.
//! 2. The archive is unpacked into a staging directory that is deleted on every
//!    way out — the content directory is untouched until every check passes.
//! 3. The staged directory's digest must equal the claimed digest. A substituted
//!    archive fails here even if every other field is perfect.
//! 4. `plugin.sig` must name exactly the claimed signer, and the signer must be
//!    in the trust root. A valid package from an untrusted identity is quarantined
//!    like any other mismatch: the signature stays meaningful because the check
//!    cannot be skipped by omitting the claim.
//! 5. The manifest inside the package must agree on name and version.
//!
//! Only then is the package promoted. The index still holds no authority: the
//! consumer re-verifies the digest after fetching, and pins it in a lockfile.

use std::sync::Arc;

use poem::http::{Method, StatusCode};
use poem::{Endpoint, Request, Response, Server, listener::TcpListener};
use semver::Version;
use serde::Deserialize;
use stanchion_dist::{Limits, Release, package, validate_name};
use stanchion_index::{DirectorySource, IndexServer};
use stanchion_index_poem::index_endpoint;
use stanchion_registry::read_manifest;

use plugin_registry::{ContentDir, display_bytes, promote};

/// Largest upload held in memory: a package bigger than this is refused unread.
const MAX_UPLOAD_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
struct State {
    content: ContentDir,
    trust_root: Vec<String>,
    source_base: String,
}

struct App {
    state: Arc<State>,
}

impl Endpoint for App {
    type Output = Response;

    async fn call(&self, request: Request) -> poem::Result<Response> {
        if request.method() == Method::POST && request.uri().path() == "/v1/upload" {
            return Ok(upload(request, &self.state).await);
        }
        // Rebuilt per request: constructing it is a directory join, and rebuilding
        // means a package promoted by an upload is served by the very next request,
        // with no cache to invalidate.
        let server = IndexServer::new(DirectorySource::new(self.state.content.path()));
        Ok(index_endpoint(server)
            .call(request)
            .await
            .unwrap_or_else(|err| {
                Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(err.to_string())
            }))
    }
}

#[derive(Debug, Deserialize)]
struct UploadQuery {
    name: String,
    version: String,
    signer: String,
    digest: String,
}

async fn upload(request: Request, state: &State) -> Response {
    let query: UploadQuery = match request.params() {
        Ok(query) => query,
        Err(err) => return refuse(StatusCode::BAD_REQUEST, &format!("query: {err}")),
    };
    match accept(state, &query, request.into_body()).await {
        Ok(release) => Response::builder()
            .status(StatusCode::CREATED)
            .content_type("application/json")
            .body(
                serde_json::json!({
                    "name": query.name,
                    "version": release.version.to_string(),
                    "digest": release.digest,
                })
                .to_string(),
            ),
        Err((status, message)) => refuse(status, &message),
    }
}

async fn accept(
    state: &State,
    query: &UploadQuery,
    body: poem::Body,
) -> Result<Release, (StatusCode, String)> {
    let name = validate_name(&query.name)
        .map_err(|err| (StatusCode::BAD_REQUEST, format!("name: {err}")))?;
    let version = Version::parse(&query.version)
        .map_err(|err| (StatusCode::BAD_REQUEST, format!("version: {err}")))?;
    let digest = query.digest.to_ascii_lowercase();
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("digest: `{}` is not a sha256 digest", query.digest),
        ));
    }
    if query.signer.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "signer: an upload with nobody to verify against is refused".to_string(),
        ));
    }

    let archive = body
        .into_vec()
        .await
        .map_err(|err| {
            (
                StatusCode::BAD_REQUEST,
                format!("reading the package: {err}"),
            )
        })
        .and_then(|bytes| {
            if bytes.len() > MAX_UPLOAD_BYTES {
                Err((
                    StatusCode::PAYLOAD_TOO_LARGE,
                    format!(
                        "the package is {}, over the {} limit",
                        display_bytes(bytes.len()),
                        display_bytes(MAX_UPLOAD_BYTES)
                    ),
                ))
            } else {
                Ok(bytes)
            }
        })?;

    // Staging lives in a temporary directory that is removed however this ends, so a
    // refused upload leaves nothing behind to be mistaken for content.
    let staging = tempfile::tempdir()
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, format!("staging: {err}")))?;
    package::unpack(archive.as_slice(), staging.path(), Limits::default()).map_err(|err| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("the archive does not unpack safely: {err}"),
        )
    })?;
    package::verify_directory(staging.path(), &format!("sha256:{digest}")).map_err(|err| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("digest mismatch, quarantined: {err}"),
        )
    })?;

    let identity = std::fs::read_to_string(staging.path().join("plugin.sig"))
        .map(|text| text.trim().to_string())
        .unwrap_or_default();
    if identity != query.signer {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            "quarantined: `plugin.sig` does not name the claimed signer".to_string(),
        ));
    }
    if !state.trust_root.iter().any(|trusted| trusted == &identity) {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("quarantined: `{identity}` is not in the trust root"),
        ));
    }

    let manifest = read_manifest(staging.path()).map_err(|reason| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("manifest: {reason}"),
        )
    })?;
    if manifest.name != name {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "quarantined: the package contains `{}`, not `{name}`",
                manifest.name
            ),
        ));
    }
    if manifest.effective_version() != version {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "quarantined: the package declares {}, not {version}",
                manifest.effective_version()
            ),
        ));
    }

    promote(
        &state.content,
        name,
        version,
        Some(&identity),
        &digest,
        &archive,
        &state.source_base,
        7,
    )
    .map_err(|err| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("promoting: {err}"),
        )
    })
}

fn refuse(status: StatusCode, message: &str) -> Response {
    println!("refused {status}: {message}");
    Response::builder()
        .status(status)
        .content_type("text/plain")
        .body(message.to_string())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let content = args
        .get(1)
        .ok_or("usage: registry-serve <content-dir> <trust-root> [addr]")?;
    let trust_path = args
        .get(2)
        .ok_or("usage: registry-serve <content-dir> <trust-root> [addr]")?;
    let addr = args
        .get(3)
        .cloned()
        .unwrap_or_else(|| "127.0.0.1:8080".to_string());

    let raw = std::fs::read_to_string(trust_path)?;
    let trust_root: Vec<String> = raw
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect();
    if trust_root.is_empty() {
        return Err("the trust root names nobody: every upload would be refused".into());
    }

    let state = Arc::new(State {
        content: ContentDir::new(content),
        trust_root,
        source_base: format!("http://{addr}"),
    });
    println!("serving {content}, uploaders trust {:?}", state.trust_root);

    Server::new(TcpListener::bind(&addr))
        .run(App { state })
        .await
        .map_err(|err| format!("serving: {err}"))?;
    Ok(())
}

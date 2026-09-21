//! The server's own behaviour: routing, freshness windows, conditional requests.
//!
//! No port is bound and no framework is involved — that is the point of keeping the
//! logic in a neutral core.

use std::fs;
use std::path::Path;
use std::time::Duration;

use http::{Method, StatusCode};
use stanchion_dist::IndexError;
use stanchion_index::{Blob, DirectorySource, IndexServer, IndexSource};
use tempfile::TempDir;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;
type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const DIGEST: &str = "sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";

/// An index on disk whose stored documents carry no expiry at all — a server is
/// expected to issue one, which is most of why it exists.
fn index_root() -> Fallible<TempDir> {
    let root = tempfile::tempdir()?;
    let v1 = root.path().join("v1");
    fs::create_dir_all(v1.join("plugins"))?;
    fs::write(
        v1.join("index.json"),
        r#"{"schema":1,"name":"acme","plugins":["formatter"]}"#,
    )?;
    fs::write(
        v1.join("plugins").join("formatter.json"),
        format!(
            r#"{{"schema":1,"name":"formatter","releases":[
                 {{"version":"1.4.2","digest":"{DIGEST}","signer":"repo:acme/plugins"}}
               ]}}"#
        ),
    )?;

    let blobs = root.path().join("blobs").join("sha256");
    fs::create_dir_all(&blobs)?;
    fs::write(blobs.join(DIGEST.trim_start_matches("sha256:")), b"package bytes")?;
    Ok(root)
}

fn server(root: &Path) -> IndexServer<DirectorySource> {
    IndexServer::new(DirectorySource::new(root)).ttl(Duration::from_secs(3600))
}

/// Consumes a response body as JSON. Bodies are readers now, so a test that wants
/// bytes says so.
fn json(served: stanchion_index::Served) -> Fallible<serde_json::Value> {
    Ok(serde_json::from_slice(&served.body.into_vec()?)?)
}

/// Reads a dotted path out of a document, so a missing field fails the test with a
/// message rather than panicking on an index.
fn field<'a>(document: &'a serde_json::Value, path: &str) -> Fallible<&'a serde_json::Value> {
    let mut node = document;
    for step in path.split('.') {
        node = match step.parse::<usize>() {
            Ok(position) => node
                .get(position)
                .ok_or_else(|| format!("`{path}`: no element {position}"))?,
            Err(_) => node.get(step).ok_or_else(|| format!("`{path}`: no key `{step}`"))?,
        };
    }
    Ok(node)
}

fn text(document: &serde_json::Value, path: &str) -> Fallible<String> {
    Ok(field(document, path)?
        .as_str()
        .ok_or_else(|| format!("`{path}` is not a string"))?
        .to_string())
}

#[test]
fn it_serves_the_two_documents_and_a_package() -> TestResult {
    let root = index_root()?;
    let server = server(root.path());

    let catalog = server.serve(&Method::GET, "/v1/index.json", None);
    assert_eq!(catalog.status, StatusCode::OK);
    assert_eq!(text(&json(catalog)?, "plugins.0")?, "formatter");

    let releases = server.serve(&Method::GET, "/v1/plugins/formatter.json", None);
    assert_eq!(releases.status, StatusCode::OK);
    assert_eq!(text(&json(releases)?, "releases.0.version")?, "1.4.2");

    let blob = server.serve(&Method::GET, &format!("/v1/blobs/{DIGEST}"), None);
    assert_eq!(blob.status, StatusCode::OK);
    assert_eq!(blob.content_length, Some(13));
    // A package arrives as a reader; consuming it here is the test's business, not
    // the server's.
    assert_eq!(blob.body.into_vec()?, b"package bytes");
    // A package is named by what it unpacks to, so its bytes cannot change.
    assert!(blob.cache_control.contains("immutable"), "{}", blob.cache_control);
    Ok(())
}

#[test]
fn it_issues_an_expiry_the_stored_documents_do_not_have() -> TestResult {
    let root = index_root()?;
    let server = server(root.path());

    // The stored document has neither field; a client's default refuses that.
    let stored = fs::read_to_string(root.path().join("v1/plugins/formatter.json"))?;
    assert!(!stored.contains("expires"));

    let served = json(server.serve(&Method::GET, "/v1/plugins/formatter.json", None))?;
    let expires = text(&served, "expires")?;
    let updated = text(&served, "updated")?;
    assert!(
        OffsetDateTime::parse(&expires, &Rfc3339)? > OffsetDateTime::parse(&updated, &Rfc3339)?
    );
    Ok(())
}

#[test]
fn responses_are_identical_within_a_window_and_change_across_one() -> TestResult {
    let root = index_root()?;
    let server = server(root.path());
    let base = OffsetDateTime::from_unix_timestamp(1_800_000_000)?;

    // Quantising to the window is what lets an ETag mean anything: a body that
    // regenerated `expires` from the wall clock would differ on every request.
    let early = server.serve_at(&Method::GET, "/v1/index.json", None, base);
    let later = server.serve_at(
        &Method::GET,
        "/v1/index.json",
        None,
        base.saturating_add(time::Duration::minutes(30)),
    );
    let next = server.serve_at(
        &Method::GET,
        "/v1/index.json",
        None,
        base.saturating_add(time::Duration::hours(2)),
    );

    let (early_etag, next_etag) = (early.etag.clone(), next.etag.clone());
    let early_bytes = early.body.into_vec()?;
    assert_eq!(early_bytes, later.body.into_vec()?, "same window, same bytes");
    assert_eq!(early_etag, later.etag);

    assert_ne!(early_bytes, next.body.into_vec()?, "a new window must refresh the expiry");
    assert_ne!(early_etag, next_etag);
    Ok(())
}

#[test]
fn a_matching_etag_becomes_a_304_with_no_body() -> TestResult {
    let root = index_root()?;
    let server = server(root.path());

    let first = server.serve(&Method::GET, "/v1/index.json", None);
    let etag = first.etag.clone().ok_or("no etag issued")?;

    let repeat = server.serve(&Method::GET, "/v1/index.json", Some(&etag));
    assert_eq!(repeat.status, StatusCode::NOT_MODIFIED);
    assert!(repeat.body.is_empty());

    let stale = server.serve(&Method::GET, "/v1/index.json", Some("\"0000000000000000\""));
    assert_eq!(stale.status, StatusCode::OK);
    assert!(!stale.body.is_empty());

    assert_eq!(
        server.serve(&Method::GET, "/v1/index.json", Some("*")).status,
        StatusCode::NOT_MODIFIED
    );
    assert_eq!(
        server.serve(&Method::GET, "/v1/index.json", Some(&format!("W/{etag}"))).status,
        StatusCode::NOT_MODIFIED
    );
    Ok(())
}

#[test]
fn a_304_cannot_outlive_the_window_that_issued_it() -> TestResult {
    let root = index_root()?;
    let server = server(root.path());
    let base = OffsetDateTime::from_unix_timestamp(1_800_000_000)?;

    let etag = server
        .serve_at(&Method::GET, "/v1/index.json", None, base)
        .etag
        .ok_or("no etag issued")?;

    // Same validator, a window later: the body changed, so the client is handed a
    // fresh document instead of being allowed to keep an expired one.
    let next = server.serve_at(
        &Method::GET,
        "/v1/index.json",
        Some(&etag),
        base.saturating_add(time::Duration::hours(2)),
    );
    assert_eq!(next.status, StatusCode::OK, "a 304 here would freeze an expired document");
    Ok(())
}

#[test]
fn an_unknown_plugin_is_404_and_nothing_else() -> TestResult {
    let root = index_root()?;
    let server = server(root.path());

    // A client maps 404 to "no such plugin" and every other status to "unreachable",
    // which is the difference between giving up and retrying.
    assert_eq!(
        server.serve(&Method::GET, "/v1/plugins/nosuch.json", None).status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        server.serve(&Method::GET, "/v1/blobs/sha256:short", None).status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        server.serve(&Method::GET, "/v1/nothing/here", None).status,
        StatusCode::NOT_FOUND
    );
    Ok(())
}

#[test]
fn a_traversing_name_is_refused_before_it_reaches_the_disk() -> TestResult {
    let root = index_root()?;
    let server = server(root.path());

    for name in ["../../etc/passwd", "..", "a/b"] {
        let served = server.serve(&Method::GET, &format!("/v1/plugins/{name}.json"), None);
        assert!(
            served.status == StatusCode::BAD_REQUEST || served.status == StatusCode::NOT_FOUND,
            "`{name}` produced {}",
            served.status
        );
        let bytes = served.body.into_vec()?;
        assert!(!bytes.windows(4).any(|w| w == b"root"), "`{name}` leaked a file");
    }
    Ok(())
}

#[test]
fn head_returns_the_headers_without_a_body() -> TestResult {
    let root = index_root()?;
    let server = server(root.path());

    let get = server.serve(&Method::GET, "/v1/index.json", None);
    let head = server.serve(&Method::HEAD, "/v1/index.json", None);
    assert_eq!(head.status, StatusCode::OK);
    assert_eq!(head.etag, get.etag);
    assert!(head.body.is_empty());
    Ok(())
}

/// A source that refuses to be read, so a passing test proves nothing was.
struct Unreadable;

impl IndexSource for Unreadable {
    fn releases(&self, name: &str) -> Result<stanchion_dist::PluginReleases, IndexError> {
        Err(IndexError::UnknownPlugin(name.to_string()))
    }

    fn blob(&self, _digest: &str) -> Result<Blob, IndexError> {
        struct Exploding;
        impl std::io::Read for Exploding {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("a HEAD must not read the package"))
            }
        }
        Ok(Blob::from_reader(Exploding, Some(4096)))
    }
}

#[test]
fn a_head_on_a_package_reports_its_length_without_reading_it() -> TestResult {
    let server = IndexServer::new(Unreadable);
    let head = server.serve(&Method::HEAD, &format!("/v1/blobs/{DIGEST}"), None);

    assert_eq!(head.status, StatusCode::OK);
    // The length comes from the source, not from reading — which the reader above
    // would have made impossible.
    assert_eq!(head.content_length, Some(4096));
    assert!(head.body.is_empty());
    Ok(())
}

#[test]
fn a_package_is_never_buffered_to_be_served() -> TestResult {
    // A reader that would be ruinous to buffer: it reports a size far larger than
    // anything we would hold in memory, and counts what is actually pulled.
    struct Counting(std::sync::Arc<std::sync::atomic::AtomicUsize>);
    impl std::io::Read for Counting {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.0.fetch_add(buf.len(), std::sync::atomic::Ordering::Relaxed);
            Ok(buf.len())
        }
    }

    struct Huge(std::sync::Arc<std::sync::atomic::AtomicUsize>);
    impl IndexSource for Huge {
        fn releases(&self, name: &str) -> Result<stanchion_dist::PluginReleases, IndexError> {
            Err(IndexError::UnknownPlugin(name.to_string()))
        }
        fn blob(&self, _digest: &str) -> Result<Blob, IndexError> {
            Ok(Blob::from_reader(
                Counting(std::sync::Arc::clone(&self.0)),
                Some(8 * 1024 * 1024 * 1024),
            ))
        }
    }

    let read = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let server = IndexServer::new(Huge(std::sync::Arc::clone(&read)));
    let served = server.serve(&Method::GET, &format!("/v1/blobs/{DIGEST}"), None);

    assert_eq!(served.content_length, Some(8 * 1024 * 1024 * 1024));
    // Building the response read nothing at all: the bytes move when the adapter
    // drains them into a socket.
    assert_eq!(read.load(std::sync::atomic::Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn other_methods_are_refused() -> TestResult {
    let root = index_root()?;
    let server = server(root.path());
    for method in [Method::POST, Method::PUT, Method::DELETE] {
        assert_eq!(
            server.serve(&method, "/v1/index.json", None).status,
            StatusCode::METHOD_NOT_ALLOWED
        );
    }
    Ok(())
}

#[test]
fn the_ttl_is_clamped_to_something_a_client_can_use() -> TestResult {
    let root = index_root()?;
    // Zero would expire every document on arrival; a year would make a yank take a
    // year to reach anyone.
    assert_eq!(
        IndexServer::new(DirectorySource::new(root.path()))
            .ttl(Duration::from_secs(0))
            .configured_ttl(),
        Duration::from_secs(60)
    );
    assert_eq!(
        IndexServer::new(DirectorySource::new(root.path()))
            .ttl(Duration::from_secs(999_999_999))
            .configured_ttl(),
        Duration::from_secs(86_400)
    );
    Ok(())
}

//! The real client against a real server.
//!
//! Everything else tests the pieces. This tests the contract: `HttpIndex` and
//! `HttpSource` are the same code a host runs, pointed at a Poem server on a real
//! socket. It is the test that catches a status code or a path the unit tests agree
//! on but the wire does not.

use std::fs;
use std::net::SocketAddr;
use std::time::Duration;

use poem::listener::TcpListener;
use poem::Server;
use semver::VersionReq;
use stanchion_dist::{Freshness, HttpIndex, HttpSource, PluginIndex, PluginSource};
use stanchion_index::{DirectorySource, IndexServer};
use stanchion_index_poem::index_endpoint;
use tempfile::TempDir;

/// `Send + Sync` because these errors cross `spawn_blocking`.
type Boxed = Box<dyn std::error::Error + Send + Sync>;
type TestResult = std::result::Result<(), Boxed>;
type Fallible<T> = std::result::Result<T, Boxed>;

const PACKAGE: &[u8] = b"a package's bytes";

/// Stored documents deliberately carry no `expires`: the server issues one, and the
/// client's default `Freshness::Required` refuses documents that have none. If the
/// server did not rewrite it, every assertion below would fail.
fn index_root() -> Fallible<(TempDir, String)> {
    let root = tempfile::tempdir()?;
    let digest = format!("sha256:{}", "ab".repeat(32));

    let v1 = root.path().join("v1");
    fs::create_dir_all(v1.join("plugins"))?;
    fs::write(
        v1.join("index.json"),
        r#"{"schema":1,"name":"example","plugins":["formatter"]}"#,
    )?;
    fs::write(
        v1.join("plugins").join("formatter.json"),
        format!(
            r#"{{"schema":1,"name":"formatter","releases":[
                 {{"version":"1.0.0","digest":"{digest}"}},
                 {{"version":"1.4.2","digest":"{digest}","signer":"repo:acme/plugins"}}
               ]}}"#
        ),
    )?;

    let blobs = root.path().join("blobs").join("sha256");
    fs::create_dir_all(&blobs)?;
    fs::write(blobs.join(digest.trim_start_matches("sha256:")), PACKAGE)?;
    Ok((root, digest))
}

/// Starts the server on an ephemeral port and returns its base URL.
async fn start(root: &TempDir) -> Fallible<String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let address: SocketAddr = listener.local_addr()?;
    drop(listener);

    let server = IndexServer::new(DirectorySource::new(root.path()))
        .ttl(Duration::from_secs(3600));

    tokio::spawn(async move {
        let _ = Server::new(TcpListener::bind(address))
            .run(index_endpoint(server))
            .await;
    });

    // Wait for the socket rather than sleeping a fixed amount.
    for _ in 0..100 {
        if std::net::TcpStream::connect(address).is_ok() {
            return Ok(format!("http://{address}"));
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Err("the server did not start".into())
}

#[tokio::test]
async fn the_real_client_resolves_against_the_real_server() -> TestResult {
    let (root, digest) = index_root()?;
    let base = start(&root).await?;

    // Blocking client, so it runs off the async worker.
    let resolved = tokio::task::spawn_blocking(move || -> Fallible<_> {
        // Default freshness: the server's issued `expires` is what makes this work.
        let index = HttpIndex::new(base.clone())?;

        let catalog = index.catalog()?;
        assert_eq!(catalog.plugins, ["formatter"]);

        let releases = index.releases("formatter")?;
        assert_eq!(releases.name, "formatter");
        assert_eq!(releases.releases.len(), 2);

        // Resolution picks by semver, not document order.
        let release = index.resolve("formatter", &VersionReq::parse("^1.0")?)?;
        assert_eq!(release.version.to_string(), "1.4.2");

        // And the package it names is fetchable from the same server.
        let bytes = HttpSource::new().fetch(&format!("{base}/v1/blobs/{}", release.digest))?;
        Ok((release.digest.clone(), bytes))
    })
    .await??;

    assert_eq!(resolved.0, digest);
    assert_eq!(resolved.1, PACKAGE);
    Ok(())
}

#[tokio::test]
async fn an_unknown_plugin_reaches_the_client_as_unknown_not_as_an_outage() -> TestResult {
    let (root, _) = index_root()?;
    let base = start(&root).await?;

    let message = tokio::task::spawn_blocking(move || -> Fallible<String> {
        let index = HttpIndex::new(base)?;
        match index.releases("nosuch") {
            Ok(_) => Err("an absent plugin must not resolve".into()),
            Err(err) => Ok(format!("{err:?}")),
        }
    })
    .await??;

    // 404 is load-bearing: anything else would surface as a transport failure, and a
    // caller would retry instead of reporting a missing plugin.
    assert!(message.contains("UnknownPlugin"), "got: {message}");
    Ok(())
}

#[tokio::test]
async fn a_client_requiring_freshness_accepts_what_the_server_issues() -> TestResult {
    let (root, _) = index_root()?;
    let base = start(&root).await?;

    tokio::task::spawn_blocking(move || -> Fallible<()> {
        // Explicit about the thing being tested: the strictest setting, against
        // documents that carry no expiry on disk.
        let index = HttpIndex::new(base)?.freshness(Freshness::Required);
        let releases = index.releases("formatter")?;
        assert!(releases.expires.is_some(), "the server must issue an expiry");
        Ok(())
    })
    .await??;
    Ok(())
}

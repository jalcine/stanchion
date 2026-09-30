//! Packs a plugin directory and publishes it into a content directory.
//!
//! ```sh
//! registry-publish ./plugins/greeter ./content --source-base http://127.0.0.1:8080
//! ```
//!
//! The plugin must carry a `plugin.sig` naming its signer: publishing records who
//! claims to have produced these bytes, and the upload endpoint later holds that
//! claim to the trust root. An unsigned directory has no claim to record, so it is
//! refused here rather than published as anonymous.

use std::error::Error;
use std::fs;
use std::path::PathBuf;

use plugin_registry::{ContentDir, display_bytes, promote};
use stanchion_dist::package;
use stanchion_registry::read_manifest;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().collect();
    let (plugin_dir, content_dir, source_base) = parse(&args)?;

    let manifest = read_manifest(&plugin_dir).map_err(|reason| format!("manifest: {reason}"))?;
    let plugin_sig = fs::read_to_string(plugin_dir.join("plugin.sig")).map_err(|err| {
        format!(
            "`{}` ships no `plugin.sig`, so there is no signer to record: {err}",
            manifest.name
        )
    })?;
    let signer = plugin_sig.trim().to_string();
    if signer.is_empty() {
        return Err(
            "`plugin.sig` is empty: a signature naming nobody verifies against nothing".into(),
        );
    }

    let mut archive = Vec::new();
    let digest = package::pack(&plugin_dir, &mut archive)?;
    println!(
        "packed `{}` {} ({})",
        manifest.name,
        manifest.effective_version(),
        display_bytes(archive.len())
    );

    let release = promote(
        &ContentDir::new(&content_dir),
        &manifest.name,
        manifest.effective_version(),
        Some(&signer),
        &digest.hex(),
        &archive,
        &source_base,
        7,
    )
    .map_err(|err| format!("promoting: {err}"))?;

    println!("digest: sha256:{}", digest.hex());
    println!("signer: {signer}");
    if let Some(source) = &release.source {
        println!("source: {source}");
    }
    println!("content: {}", content_dir.display());
    Ok(())
}

fn parse(args: &[String]) -> Result<(PathBuf, PathBuf, String), Box<dyn Error>> {
    let mut positional: Vec<&str> = Vec::new();
    let mut source_base = "http://127.0.0.1:8080".to_string();
    let mut rest = args.get(1..).unwrap_or(&[]).iter().peekable();
    while let Some(arg) = rest.next() {
        if *arg == "--source-base" {
            let value = rest.next().ok_or("--source-base needs a value")?;
            source_base = (*value).to_string();
        } else {
            positional.push(arg);
        }
    }
    let plugin = positional
        .first()
        .ok_or("usage: registry-publish <plugin-dir> <content-dir> [--source-base URL]")?;
    let content = positional
        .get(1)
        .ok_or("usage: registry-publish <plugin-dir> <content-dir> [--source-base URL]")?;
    Ok((PathBuf::from(plugin), PathBuf::from(content), source_base))
}

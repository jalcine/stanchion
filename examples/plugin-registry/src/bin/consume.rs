//! Installs a plugin from the registry and runs it.
//!
//! ```sh
//! registry-consume http://127.0.0.1:8080 ./host greeter '^1.0' --yes
//! ```
//!
//! Resolution goes through the index, but nothing the index says is believed:
//! the package is unpacked, its digest compared to the release pin, and its
//! manifest checked for name and version before anything reaches `./host`.
//! A lockfile pin, once written, overrides the index on every later run —
//! upgrading is a deliberate act, not a side effect of asking.
//!
//! Staging also produces an upgrade review. A first install always counts as
//! widening (there is nothing to compare against), so without `--yes` this
//! commits nothing: the review is printed and the staging is discarded.

use std::error::Error;
use std::path::PathBuf;

use semver::VersionReq;
use stanchion_dist::{HttpIndex, HttpSource, Installer};
use stanchion_lua::backend::LuaBackend;
use stanchion_lua::sandbox::Sandbox;
use stanchion_registry::{Lockfile, Registry, Value};

struct Args {
    base: String,
    root: PathBuf,
    name: String,
    requirement: VersionReq,
    yes: bool,
}

fn main() -> std::result::Result<(), Box<dyn Error>> {
    let argv: Vec<String> = std::env::args().collect();
    let args = parse(&argv)?;

    let index = HttpIndex::new(args.base)?;
    // Plain HTTP is refused unless opted into: the index picks these URLs, so a
    // hostile one must not be able to downgrade a fetch. Localhost here is the
    // trusted mirror this example runs; integrity still comes from the digest.
    let source = HttpSource::new().allow_http(true);
    let installer = Installer::new(index, source, &args.root);

    let lock_path = args.root.join("stanchion.lock");
    let mut lockfile = Lockfile::load_or_empty(&lock_path)?;
    let staged = installer
        .stage(&args.name, &args.requirement, &lockfile)
        .map_err(|err| format!("staging: {err}"))?;
    println!(
        "staged {} {} (sha256:{})",
        staged.name(),
        staged.release().version,
        staged.digest().hex()
    );
    match staged.review() {
        Some(review) if review.widens() => println!("review: this widens authority: {review:?}"),
        Some(review) => println!("review: narrows or holds: {review:?}"),
        None => println!("review: first install, which always counts as widening"),
    }
    if staged.widens() && !args.yes {
        staged.discard()?;
        return Err("refusing to commit a widening install without --yes".into());
    }
    let pin = staged.commit()?;
    lockfile.pin(args.name.clone(), pin);
    lockfile.save(&lock_path)?;
    println!("installed to {}", args.root.display());

    let mut registry = Registry::new().with_runtime(Box::new(LuaBackend::isolated(
        Sandbox::restricted(),
    )));
    let report = registry.load_dir(&args.root)?;
    if !report.failures.is_empty() {
        for failure in &report.failures {
            println!("load failed: {} — {}", failure.name, failure.reason);
        }
        return Err("the installed plugin does not load".into());
    }
    let name = registry
        .get(&args.name)
        .ok_or_else(|| format!("`{}` is installed but not loaded", args.name))?
        .name()
        .to_string();
    match registry.call(&name, "greet", &[]) {
        Ok(Value::Str(text)) => println!("{name} says: {text}"),
        Ok(other) => println!("{name} answered unexpectedly: {other:?}"),
        Err(err) => return Err(format!("greet failed: {err}").into()),
    }
    Ok(())
}

fn parse(args: &[String]) -> std::result::Result<Args, Box<dyn Error>> {
    let mut positional: Vec<&str> = Vec::new();
    let mut yes = false;
    for arg in args.get(1..).unwrap_or(&[]) {
        if arg == "--yes" {
            yes = true;
        } else {
            positional.push(arg);
        }
    }
    let usage = "usage: registry-consume <index-base> <plugin-root> <name> <requirement> [--yes]";
    let base = positional.first().ok_or(usage)?.to_string();
    let root = PathBuf::from(positional.get(1).ok_or(usage)?);
    let name = positional.get(2).ok_or(usage)?.to_string();
    let requirement: VersionReq = positional.get(3).ok_or(usage)?.parse()?;
    Ok(Args {
        base,
        root,
        name,
        requirement,
        yes,
    })
}

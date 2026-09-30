//! Watches a plugin directory, reloads on change, and smoke-tests the result.
//!
//! ```sh
//! harness-watch ./plugins counter
//! ```
//!
//! The loop polls the directory digest twice a second — no file-watching
//! dependency, nothing platform-specific. On change it calls `reload`, which
//! instantiates the new code beside the old and swaps only on success: a broken
//! edit is refused and the previous instance keeps answering, which the smoke
//! call after every reload demonstrates.

use std::error::Error;
use std::path::PathBuf;
use std::time::Duration;

use stanchion_lua::backend::LuaBackend;
use stanchion_lua::sandbox::Sandbox;
use stanchion_registry::{DirectoryDigest, Registry, Value};

fn main() -> std::result::Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().collect();
    let root = args
        .get(1)
        .ok_or("usage: harness-watch <plugins-dir> <name>")?;
    let name = args
        .get(2)
        .ok_or("usage: harness-watch <plugins-dir> <name>")?;
    let root = PathBuf::from(root);

    let mut registry = Registry::new().with_runtime(Box::new(LuaBackend::isolated(
        Sandbox::restricted(),
    )));
    let report = registry.load_dir(&root)?;
    for failure in &report.failures {
        println!("load failed: {} — {}", failure.name, failure.reason);
    }
    if !report.loaded.iter().any(|loaded| loaded == name) {
        return Err(format!("`{name}` did not load").into());
    }
    smoke(&registry, name)?;

    let mut digest = DirectoryDigest::compute(&root)?;
    println!("watching {} — edit and save to reload", root.display());
    loop {
        std::thread::sleep(Duration::from_millis(500));
        let current = DirectoryDigest::compute(&root)?;
        if current.hex() == digest.hex() {
            continue;
        }
        digest = current;
        match registry.reload(name) {
            Ok(()) => {
                println!("reloaded `{name}`");
                smoke(&registry, name)?;
            }
            Err(err) => {
                println!("reload refused ({err}); the previous instance keeps serving:");
                smoke(&registry, name)?;
            }
        }
    }
}

/// Calls the plugin and prints what it says. The point is what this proves after
/// a reload: the new code answering, or the old code surviving a refused one.
fn smoke(registry: &Registry, name: &str) -> std::result::Result<(), Box<dyn Error>> {
    let plugin = registry
        .get(name)
        .ok_or_else(|| format!("`{name}` is not loaded"))?;
    match registry.call(name, "describe", &[]) {
        Ok(Value::Str(text)) => println!("{} says: {text}", plugin.name()),
        Ok(other) => println!("{} answered unexpectedly: {other:?}", plugin.name()),
        Err(err) => println!("{} stopped: {err}", plugin.name()),
    }
    Ok(())
}

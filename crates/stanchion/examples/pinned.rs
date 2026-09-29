//! The lockfile is the trust anchor: pin bytes, refuse substitution, review upgrades.
//!
//! A digest pins the exact build that may load — no signing infrastructure needed.
//! Tampering with a byte fails the load, an unpinned sibling fails too, and an
//! upgrade that asks for more authority is reported before anything runs.
//!
//! ```sh
//! cargo run -p stanchion --features lua54,vendored,signatures --example pinned
//! ```

use std::fs;

use stanchion::registry::{LockedPlugin, Lockfile, Manifest, Registry, UpgradeReview};
use stanchion_lua::lua_class;
use stanchion_lua::mlua::{Lua, Result, Table};

#[lua_class]
pub trait Task {
    fn new(config: Table, deps: Table) -> Result<Self>;
    fn run(&self) -> Result<String>;
}

const PLUGIN_TOML: &str = r#"
name = "pinned-demo"
version = "1.0.0"
"#;

const INIT_LUA: &str = r#"
local Demo = {}
Demo.__index = Demo

function Demo.new(config, deps) return setmetatable({}, Demo) end
function Demo:run() return "v1" end

return Demo
"#;

const MANIFEST_V1: &str = r#"
name = "pinned-demo"
version = "1.0.0"
"#;

const MANIFEST_V2: &str = r#"
name = "pinned-demo"
version = "1.1.0"

[capabilities.kv]
namespace = "tenant-7"
"#;

fn write_plugin(dir: &std::path::Path) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    fs::write(dir.join("plugin.toml"), PLUGIN_TOML)?;
    fs::write(dir.join("init.lua"), INIT_LUA)?;
    Ok(())
}

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("plugins");
    write_plugin(&root.join("pinned-demo"))?;

    // Pin the bytes that are on disk right now. Nothing else is trusted afterwards:
    // not the directory, not a mirror, not an index.
    let digest = stanchion::registry::DirectoryDigest::compute(&root.join("pinned-demo"))?;
    let mut lockfile = Lockfile::new();
    lockfile.pin("pinned-demo", LockedPlugin::from_digest(&digest));
    println!("pinned: sha256:{}", digest.hex());

    let mut registry: Registry<TaskClass> =
        Registry::new(Lua::new()).with_lockfile(lockfile.clone());
    let report = registry.load_dir(&root)?;
    println!("loaded: {:?}", report.loaded);
    if let Some(plugin) = registry.get("pinned-demo") {
        println!("run: {}", plugin.instance().run()?);
    }

    // A byte changes: the pin no longer matches, so the plugin fails instead of
    // running something nobody reviewed.
    fs::write(
        root.join("pinned-demo").join("init.lua"),
        format!("{INIT_LUA}\n-- tampered\n"),
    )?;
    let mut tampered: Registry<TaskClass> =
        Registry::new(Lua::new()).with_lockfile(lockfile.clone());
    let report = tampered.load_dir(&root)?;
    println!("\nafter tampering, loaded: {:?}", report.loaded);
    for failure in &report.failures {
        println!(
            "refused: {} — {}",
            failure.name,
            first_line(&failure.reason.to_string())
        );
    }

    // Restore the pinned bytes, then add a sibling nobody pinned: appearing beside
    // pinned plugins is exactly the event worth refusing.
    write_plugin(&root.join("pinned-demo"))?;
    write_plugin(&root.join("extra"))?;
    fs::write(
        root.join("extra").join("plugin.toml"),
        "name = \"extra\"\nversion = \"1.0.0\"\n",
    )?;
    let mut with_sibling: Registry<TaskClass> = Registry::new(Lua::new()).with_lockfile(lockfile);
    let report = with_sibling.load_dir(&root)?;
    println!("\nwith sibling, loaded: {:?}", report.loaded);
    for failure in &report.failures {
        println!(
            "refused: {} — {}",
            failure.name,
            first_line(&failure.reason.to_string())
        );
    }

    // Upgrade review without running any code: the candidate asks for `kv` the
    // installed version never had, so this widens and needs a person.
    let installed: Manifest = stanchion::registry::toml::from_str(MANIFEST_V1)?;
    let candidate: Manifest = stanchion::registry::toml::from_str(MANIFEST_V2)?;
    let review = UpgradeReview::between(&installed, &candidate);
    println!("\n{review}widens: {}", review.widens());

    Ok(())
}

fn first_line(message: &str) -> &str {
    message.lines().next().unwrap_or(message)
}

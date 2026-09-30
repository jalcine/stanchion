//! `[rocks]` are fail-closed: declared but unresolvable means not loaded.
//!
//! A plugin needing an external Lua library says so in its manifest. Without a
//! configured tree the registry refuses it rather than resolving `require` from
//! whatever happens to be on the machine — and loading never installs, so this
//! touches no network. Provisioning stays an explicit `install_rocks` call.
//!
//! ```sh
//! cargo run -p stanchion --features lua54,vendored,luarocks --example rocks
//! ```

use std::fs;

use stanchion::registry::{Registry, read_manifest, rocks::RocksConfig};
use stanchion_lua::backend::LuaBackend;

const PLUGIN_TOML: &str = r#"
name = "needs-json"
version = "1.0.0"

[rocks]
dkjson = "2.11"
"#;

const INIT_LUA: &str = r#"
local NeedsJson = {}
NeedsJson.__index = NeedsJson

function NeedsJson.new(config, deps) return setmetatable({}, NeedsJson) end
function NeedsJson:run() return "parsed" end

return NeedsJson
"#;

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("plugins");
    fs::create_dir_all(root.join("needs-json"))?;
    fs::write(root.join("needs-json").join("plugin.toml"), PLUGIN_TOML)?;
    fs::write(root.join("needs-json").join("init.lua"), INIT_LUA)?;

    // Static review first: what the plugin wants, without running any of its code.
    let manifest = read_manifest(&root.join("needs-json"))
        .map_err(|err| std::io::Error::other(format!("the fixture manifest is invalid: {err}")))?;
    let rocks: Vec<&str> = manifest.rocks.keys().map(String::as_str).collect();
    println!("declares rocks: {rocks:?}");

    // No tree configured: refused rather than resolved from the machine.
    let mut bare = Registry::new().with_runtime(Box::new(LuaBackend::shared()));
    let report = bare.load_dir(&root)?;
    println!("without a tree, loaded: {:?}", report.loaded);
    for failure in &report.failures {
        println!(
            "refused: {} — {}",
            failure.name,
            first_line(&failure.reason.to_string())
        );
    }

    // A tree is configured but holds no `dkjson`: still refused, still no network.
    // (`install_rocks` is the only call that shells out to `luarocks`, and nothing
    // here invokes it.)
    let tree = temp.path().join("rocks-tree");
    fs::create_dir_all(&tree)?;
    let mut with_tree = Registry::new()
        .with_runtime(Box::new(LuaBackend::shared()))
        .with_rocks(RocksConfig::new(&tree));
    let report = with_tree.load_dir(&root)?;
    println!("\nwith an empty tree, loaded: {:?}", report.loaded);
    for failure in &report.failures {
        println!(
            "refused: {} — {}",
            failure.name,
            first_line(&failure.reason.to_string())
        );
    }

    Ok(())
}

fn first_line(message: &str) -> &str {
    message.lines().next().unwrap_or(message)
}

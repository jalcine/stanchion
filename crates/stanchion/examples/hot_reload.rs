//! Reload propagates through a dependency chain without rebuilding dependents.
//!
//! `formatter` publishes `exports`; `counter` is wired to that proxy. Editing the
//! provider and calling `reload` repoints the proxy, so the dependent sees the new
//! surface. A reload that fails leaves the old instance in place.
//!
//! The fixtures are copied into a temp dir so the example never mutates the repo.
//!
//! ```sh
//! cargo run -p stanchion --features lua54,vendored,registry --example hot_reload
//! ```

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use stanchion::registry::Registry;
use stanchion_lua::lua_class;
use stanchion_lua::mlua::{Lua, Result, Table};

/// Every plugin in this bus implements this.
#[lua_class]
pub trait Handler {
    fn new(config: Table, deps: Table) -> Result<Self>;
    fn name(&self) -> Result<String>;
    fn handle(&self, kind: String, payload: String) -> Result<String>;
}

const FORMATTER_V2: &str = r#"
local Formatter = {}
Formatter.__index = Formatter

function Formatter.new(config, deps)
  return setmetatable({ prefix = config.prefix }, Formatter)
end

function Formatter:name() return "formatter" end

function Formatter:handle(kind, payload)
  return self.prefix .. " " .. kind .. ": " .. payload
end

function Formatter:exports()
  return {
    decorate = function(text) return "[v2] " .. text end,
  }
end

return Formatter
"#;

const FORMATTER_BROKEN: &str = "this is not lua (\n";

fn fixture_root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/plugins"))
}

fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

fn counter_line(registry: &Registry<HandlerClass>) -> String {
    for outcome in registry.dispatch(|plugin| plugin.handle("deploy".into(), "v1".into())) {
        if outcome.name == "counter" {
            return outcome.result.map_err(|err| err.to_string()).map_or_else(
                |err| format!("counter failed: {}", first_line(&err)),
                |text| text,
            );
        }
    }
    "counter produced nothing".to_string()
}

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("plugins");
    copy_dir(&fixture_root().join("formatter"), &root.join("formatter"))?;
    copy_dir(&fixture_root().join("counter"), &root.join("counter"))?;

    let mut registry: Registry<HandlerClass> = Registry::new(Lua::new());
    let report = registry.load_dir(&root)?;
    println!("loaded: {:?}", report.loaded);

    println!("before reload: {}", counter_line(&registry));

    // Editing the provider and reloading repoints the exports proxy: `counter` is
    // never rebuilt, yet its next call sees the new decoration.
    fs::write(root.join("formatter").join("init.lua"), FORMATTER_V2)?;
    registry.reload("formatter")?;
    println!("after reload:  {}", counter_line(&registry));

    // A reload that fails is fail-closed: the old instance keeps serving.
    fs::write(root.join("formatter").join("init.lua"), FORMATTER_BROKEN)?;
    match registry.reload("formatter") {
        Ok(()) => println!("unexpected: a broken chunk reloaded"),
        Err(err) => println!("broken reload refused: {}", first_line(&err.to_string())),
    }
    println!("still serving: {}", counter_line(&registry));

    Ok(())
}

fn first_line(message: &str) -> &str {
    message.lines().next().unwrap_or(message)
}

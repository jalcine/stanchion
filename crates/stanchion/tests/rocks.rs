//! LuaRocks integration, exercised against a rock built offline with `luarocks make`.
#![cfg(feature = "luarocks")]

use std::fs;
use std::path::Path;
use std::process::Command;

use stanchion::lua_class;
use stanchion::mlua::{Lua, Result};
use stanchion::registry::rocks::RocksConfig;
use stanchion::registry::{FailureReason, Registry};
use tempfile::TempDir;

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;
type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[lua_class]
pub trait Greeter {
    fn new(config: stanchion::mlua::Table, deps: stanchion::mlua::Table) -> Result<Self>;
    fn greet(&self, who: String) -> Result<String>;
}

const USES_ROCK: &str = r#"
local rock = require("fakerock")

local P = {}
P.__index = P
function P.new(config, deps) return setmetatable({}, P) end
function P:greet(who) return rock.greet(who) end
return P
"#;

/// LuaRocks is an optional tool; skip rather than fail where it is absent.
fn luarocks_available() -> bool {
    Command::new("luarocks")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

/// Builds a pure-Lua rock into `tree` from a local rockspec, with no network access.
fn build_fake_rock(workdir: &Path, tree: &Path, version: &str) -> TestResult {
    fs::write(
        workdir.join("fakerock.lua"),
        "return { greet = function(who) return \"fakerock greets \" .. who end }\n",
    )?;
    let spec_name = format!("fakerock-{version}.rockspec");
    fs::write(
        workdir.join(&spec_name),
        format!(
            "package = \"fakerock\"\n\
             version = \"{version}\"\n\
             source = {{ url = \"file://.\" }}\n\
             description = {{ summary = \"offline test rock\" }}\n\
             build = {{ type = \"builtin\", modules = {{ fakerock = \"fakerock.lua\" }} }}\n"
        ),
    )?;

    let output = Command::new("luarocks")
        .current_dir(workdir)
        .arg("--tree")
        .arg(tree)
        .args(["--lua-version", "5.4", "make", &spec_name])
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "luarocks make failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(())
}

fn write_plugin(root: &Path, name: &str, manifest: &str, source: &str) -> TestResult {
    let dir = root.join(name);
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("plugin.toml"), manifest)?;
    fs::write(dir.join("init.lua"), source)?;
    Ok(())
}

/// A plugin root plus a rocks tree holding `fakerock 1.2-1`.
fn fixture(requirement: &str) -> Fallible<Option<(TempDir, TempDir)>> {
    if !luarocks_available() {
        return Ok(None);
    }
    let work = tempfile::tempdir()?;
    let tree = tempfile::tempdir()?;
    build_fake_rock(work.path(), tree.path(), "1.2-1")?;

    let root = tempfile::tempdir()?;
    write_plugin(
        root.path(),
        "rocky",
        &format!("name = \"rocky\"\n\n[rocks]\nfakerock = \"{requirement}\"\n"),
        USES_ROCK,
    )?;
    Ok(Some((root, tree)))
}

fn first_failure(report: &stanchion::registry::LoadReport) -> Fallible<&FailureReason> {
    report
        .failures
        .first()
        .map(|failure| &failure.reason)
        .ok_or_else(|| "expected the plugin to fail".into())
}

#[test]
fn a_declared_rock_is_loadable_from_the_tree() -> TestResult {
    let Some((root, tree)) = fixture("1.2")? else {
        eprintln!("skipping: luarocks not installed");
        return Ok(());
    };

    let mut registry: Registry<GreeterClass> =
        Registry::new(Lua::new()).with_rocks(RocksConfig::new(tree.path()));
    let report = registry.load_dir(root.path())?;

    assert_eq!(report.loaded, ["rocky"], "failures: {:?}", report.failures);
    let rocky = registry.get("rocky").ok_or("rocky should be loaded")?;
    assert_eq!(rocky.instance().greet("you".to_string())?, "fakerock greets you");
    Ok(())
}

#[test]
fn a_bare_requirement_accepts_the_rockspec_revision() -> TestResult {
    // The tree holds `1.2-1`; the manifest asks for `1.2`.
    let Some((root, tree)) = fixture("1.2")? else {
        eprintln!("skipping: luarocks not installed");
        return Ok(());
    };
    let mut registry: Registry<GreeterClass> =
        Registry::new(Lua::new()).with_rocks(RocksConfig::new(tree.path()));
    assert!(registry.load_dir(root.path())?.is_clean());
    Ok(())
}

#[test]
fn an_unsatisfied_version_is_reported() -> TestResult {
    let Some((root, tree)) = fixture(">= 2.0")? else {
        eprintln!("skipping: luarocks not installed");
        return Ok(());
    };

    let mut registry: Registry<GreeterClass> =
        Registry::new(Lua::new()).with_rocks(RocksConfig::new(tree.path()));
    let report = registry.load_dir(root.path())?;

    assert!(report.loaded.is_empty());
    assert!(
        matches!(
            first_failure(&report)?,
            FailureReason::IncompatibleRock { name, found, .. }
                if name == "fakerock" && found == "1.2-1"
        ),
        "got: {:?}",
        report.failures
    );
    Ok(())
}

#[test]
fn a_missing_rock_is_reported() -> TestResult {
    if !luarocks_available() {
        eprintln!("skipping: luarocks not installed");
        return Ok(());
    }
    let tree = tempfile::tempdir()?;
    let root = tempfile::tempdir()?;
    write_plugin(
        root.path(),
        "rocky",
        "name = \"rocky\"\n\n[rocks]\nfakerock = \"*\"\n",
        USES_ROCK,
    )?;

    let mut registry: Registry<GreeterClass> =
        Registry::new(Lua::new()).with_rocks(RocksConfig::new(tree.path()));
    let report = registry.load_dir(root.path())?;

    assert!(
        matches!(
            first_failure(&report)?,
            FailureReason::MissingRock { name, .. } if name == "fakerock"
        ),
        "got: {:?}",
        report.failures
    );
    Ok(())
}

#[test]
fn declaring_rocks_without_a_tree_fails_loudly() -> TestResult {
    let root = tempfile::tempdir()?;
    write_plugin(
        root.path(),
        "rocky",
        "name = \"rocky\"\n\n[rocks]\nfakerock = \"*\"\n",
        USES_ROCK,
    )?;

    // No `with_rocks`: the plugin must not silently resolve from the machine.
    let mut registry: Registry<GreeterClass> = Registry::new(Lua::new());
    let report = registry.load_dir(root.path())?;

    assert!(report.loaded.is_empty());
    let FailureReason::Rocks(message) = first_failure(&report)? else {
        return Err(format!("expected a rocks failure, got {:?}", report.failures).into());
    };
    assert!(message.contains("no LuaRocks tree configured"), "got: {message}");
    Ok(())
}

#[test]
fn a_malformed_requirement_is_a_manifest_error() -> TestResult {
    let root = tempfile::tempdir()?;
    write_plugin(
        root.path(),
        "rocky",
        "name = \"rocky\"\n\n[rocks]\nfakerock = \">=\"\n",
        USES_ROCK,
    )?;

    let mut registry: Registry<GreeterClass> = Registry::new(Lua::new());
    let report = registry.load_dir(root.path())?;

    assert!(
        matches!(first_failure(&report)?, FailureReason::Manifest(message)
            if message.contains("names an operator but no version")),
        "got: {:?}",
        report.failures
    );
    Ok(())
}

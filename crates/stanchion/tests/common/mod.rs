//! Shared test fixtures for `stanchion/tests/` — eliminates duplication across 26 test families.
//!
//! Consolidates the most frequently repeated test scaffolding:
//! - `write_plugin` – creates a temporary plugin (TOML + init.lua)
//! - `first_failure` – extracts the first failure reason from a `LoadReport`
//! - `probe_source` – template for a minimal Lua plugin source
//! - `plugin_root` – helper to obtain a fresh `TempDir` for plugin testing

use std::fs;
use std::path::Path;

use stanchion::lua_class;
use stanchion::mlua::Lua, Result, Table;
use stanchion::registry::{
    FailureReason,
    LoadReport,
    Registry,
    Sandbox,
};
use tempfile::TempDir;

/// Creates a temporary plugin directory with a TOML manifest and a Lua init file.
///
/// This is the core fixture used by `standchion/tests/` (remote, sandbox, rocks, etc.) to
/// spin up isolated plugin environments for testing.
fn write_plugin(
    root: &Path,
    name: &str,
    manifest: &str,
    source: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let dir = root.join(name);
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("plugin.toml"), manifest)?;
    fs::write(dir.join("init.lua"), source)?;
    Ok(())
}

/// Extracts the first failure reason from a `LoadReport`.
///
/// Used by tests that expect a plugin to fail (e.g., malformed manifest, missing deps).
fn first_failure(report: &LoadReport) -> Result<&FailureReason, Box<dyn std::error::Error>> {
    report
        .failures
        .first()
        .map(|f| &f.reason)
        .ok_or_else(|| "expected the plugin to fail".into())
}

/// Generates a minimal Lua plugin source that implements the `Probe` trait.
///
/// This is the canonical template for any plugin that needs to be tested in the
/// `standchion/tests/` suite. All other plugin tests derive from this.
fn probe_source(body: &str) -> String {
    format!(
        r#"
local P = {}
P.__index = P

function P.new(config: Table, deps: Table) -> Result<Self> {
    return setmetatable({}, P)
}

function P:run(input: String) -> Result<String> {
    {body}
end
return P
"
    )
}

/// Obtains a fresh temporary directory suitable for plugin testing.
///
/// Returns `Ok(TempDir)` for every call — no cleanup required (the caller owns the dir).
fn plugin_root() -> Result<TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    Ok(root)
}

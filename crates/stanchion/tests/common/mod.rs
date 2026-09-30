//! Shared test fixtures for `stanchion/tests/`.
//!
//! Each integration test is its own crate, so a test that includes this with
//! `mod common;` only uses some of it; `dead_code` is allowed for the rest.
#![allow(dead_code)]

use std::fs;
use std::path::Path;

use stanchion::registry::{FailureReason, LoadReport, Registry};
use stanchion_lua::backend::LuaBackend;
use stanchion_lua::sandbox::Sandbox;

pub type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;
pub type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// A registry with one isolated Lua backend under a restricted sandbox.
pub fn registry() -> Registry {
    Registry::new().with_runtime(Box::new(LuaBackend::isolated(Sandbox::restricted())))
}

/// Creates `<root>/<name>/` holding `plugin.toml` and `init.lua`.
pub fn write_plugin(root: &Path, name: &str, manifest: &str, source: &str) -> TestResult {
    let dir = root.join(name);
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("plugin.toml"), manifest)?;
    fs::write(dir.join("init.lua"), source)?;
    Ok(())
}

/// The reason the first plugin in `report` failed, or an error if none did.
pub fn first_failure(report: &LoadReport) -> Fallible<&FailureReason> {
    report
        .failures
        .first()
        .map(|failure| &failure.reason)
        .ok_or_else(|| "expected the plugin to fail".into())
}

/// A plugin class whose `run(input)` method executes `body`.
///
/// Dependencies are kept on the instance (`self.deps`), so bodies may read
/// `self.deps.<name>` like a wired plugin would.
pub fn probe_source(body: &str) -> String {
    format!(
        "local P = {{}}\nP.__index = P\n\
         function P.new(config, deps) return setmetatable({{deps = deps}}, P) end\n\
         function P:run(input)\n  {body}\nend\nreturn P\n"
    )
}

/// Calls `run` on the `probe` plugin, expecting a string result.
pub fn run(registry: &Registry, input: &str) -> Fallible<String> {
    run_named(registry, "probe", input)
}

/// Calls `run` on the named plugin, expecting a string result.
pub fn run_named(registry: &Registry, plugin: &str, input: &str) -> Fallible<String> {
    match registry.call(plugin, "run", &[stanchion_abi::Value::Str(input.to_string())])? {
        stanchion_abi::Value::Str(text) => Ok(text),
        other => Err(format!("expected a string, got {other:?}").into()),
    }
}

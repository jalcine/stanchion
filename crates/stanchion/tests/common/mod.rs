//! Shared test fixtures for `stanchion/tests/`.
//!
//! Each integration test is its own crate, so a test that includes this with
//! `mod common;` only uses some of it; `dead_code` is allowed for the rest.
#![allow(dead_code)]

use std::fs;
use std::path::Path;

use stanchion::registry::{FailureReason, LoadReport, Runtime};
use stanchion_lua::mlua::{self, FromLuaMulti, IntoLuaMulti, Lua};

pub type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;
pub type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

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
/// Matches a `Probe` trait with `new(config, deps)` and `run(&self, input: String)`.
pub fn probe_source(body: &str) -> String {
    format!(
        "local P = {{}}\nP.__index = P\n\
         function P.new(config, deps) return setmetatable({{}}, P) end\n\
         function P:run(input)\n  {body}\nend\nreturn P\n"
    )
}

/// Runs `body` against the plugin state behind a provider's or installer's runtime.
pub fn with_lua<T>(
    runtime: &dyn Runtime,
    body: impl FnOnce(&Lua) -> mlua::Result<T>,
) -> stanchion_abi::Result<T> {
    let state = runtime
        .lua_state()
        .ok_or_else(|| stanchion_abi::Error::Config("a Lua runtime is required".to_string()))?;
    let lua = state
        .lock()
        .map_err(|_| stanchion_abi::Error::Config("the Lua state is poisoned".to_string()))?;
    body(&lua).map_err(|err| stanchion_abi::Error::Config(err.to_string()))
}

/// Hands a Rust closure to a plugin from a capability provider.
///
/// Providers return a `stanchion_abi::Value`; a Lua function crosses that boundary
/// by being created on the plugin's state and parked in the ABI's function cache.
pub fn lua_function<A, R, F>(
    runtime: &dyn Runtime,
    func: F,
) -> stanchion_abi::Result<stanchion_abi::Value>
where
    A: FromLuaMulti,
    R: IntoLuaMulti,
    F: Fn(&Lua, A) -> mlua::Result<R> + Send + 'static,
{
    with_lua(runtime, |lua| {
        let function = lua.create_function(func)?;
        Ok(stanchion_abi::value::lua::lua_to_abi(
            lua,
            &mlua::Value::Function(function),
        ))
    })
}

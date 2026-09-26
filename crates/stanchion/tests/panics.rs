//! A Rust panic raised under a plugin call is reported, not propagated.
//!
//! These tests deliberately panic, so each one prints a panic message and backtrace to
//! stderr on the way through. That output is the panic being handled, not a failure.
#![cfg(feature = "registry")]

mod common;

use std::fs;

use stanchion::registry::{FailureReason, Panicked, Registry, Rules, Sandbox};
use stanchion_lua::lua_class;
use stanchion_lua::mlua::{Lua, Result, Table};

use common::lua_function;

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

#[lua_class]
pub trait Probe {
    fn new(config: Table, deps: Table) -> Result<Self>;
    fn run(&self, input: String) -> Result<String>;
}

/// A host whose one capability panics when a plugin calls it.
#[expect(
    clippy::panic,
    reason = "the panic is the subject under test, not a fallible path taking a shortcut"
)]
fn exploding_registry() -> Registry<ProbeClass> {
    Registry::isolated(Lua::new(), Sandbox::restricted()).with_setup(|host| {
        host.capability("boom", |runtime, _grant| {
            lua_function(runtime, |_, ()| -> Result<()> {
                panic!("the provider gave up")
            })
        });
        Ok(())
    })
    .with_policy(Rules::deny_all().allow("boom"))
}

fn write_plugin(root: &std::path::Path, body: &str, manifest: &str) -> TestResult {
    let dir = root.join("probe");
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("plugin.toml"), manifest)?;
    fs::write(
        dir.join("init.lua"),
        format!(
            "local P = {{}}\nP.__index = P\n\
             function P.new(config, deps) return setmetatable({{}}, P) end\n\
             function P:run(input)\n  {body}\nend\nreturn P\n"
        ),
    )?;
    Ok(())
}

/// Recovers a [`Panicked`] from wherever mlua wrapped it.
///
/// `Error::downcast_ref` descends through the `CallbackError` and `WithContext` layers
/// mlua adds on the way out, which a plain `source()` walk does not: mlua's own
/// `source` deliberately skips the external error it holds.
fn panicked_in(error: &stanchion_lua::mlua::Error) -> Option<&Panicked> {
    error.downcast_ref::<Panicked>()
}

#[test]
fn a_panicking_capability_fails_the_call_not_the_caller() -> TestResult {
    let root = tempfile::tempdir()?;
    write_plugin(
        root.path(),
        "boom()\n  return \"unreachable\"",
        "name = \"probe\"\n\n[capabilities]\nboom = {}\n",
    )?;

    let mut registry = exploding_registry();
    assert!(registry.load_dir(root.path())?.is_clean());

    // The call fails; reaching this line at all is the point.
    let outcomes = registry.dispatch(|probe| probe.run(String::new()));
    let outcome = outcomes.first().ok_or("one plugin was dispatched to")?;
    let error = outcome
        .result
        .as_ref()
        .err()
        .ok_or("the call should have failed")?;

    let panicked = panicked_in(error).ok_or_else(|| format!("not a panic: {error}"))?;
    assert!(
        panicked.message().contains("the provider gave up"),
        "got: {}",
        panicked.message()
    );
    Ok(())
}

#[test]
fn a_panic_during_load_fails_only_that_plugin() -> TestResult {
    let root = tempfile::tempdir()?;
    // The constructor calls it, so the panic happens while loading.
    write_plugin(
        root.path(),
        "return \"unused\"",
        "name = \"probe\"\n\n[capabilities]\nboom = {}\n",
    )?;
    fs::write(
        root.path().join("probe/init.lua"),
        "local P = {}\nP.__index = P\n\
         function P.new(config, deps) boom() return setmetatable({}, P) end\n\
         function P:run(input) return \"ok\" end\nreturn P\n",
    )?;

    let mut registry = exploding_registry();
    let report = registry.load_dir(root.path())?;

    assert!(report.loaded.is_empty(), "got: {:?}", report.loaded);
    let failure = report.failures.first().ok_or("the plugin should fail")?;
    assert!(
        matches!(&failure.reason, FailureReason::Panicked(_)),
        "got: {}",
        failure.reason
    );
    assert!(
        failure.reason.to_string().contains("the provider gave up"),
        "got: {}",
        failure.reason
    );
    Ok(())
}

#[test]
fn an_ordinary_lua_error_is_still_an_ordinary_error() -> TestResult {
    let root = tempfile::tempdir()?;
    write_plugin(
        root.path(),
        "error(\"just a failure\")",
        "name = \"probe\"\n",
    )?;

    let mut registry = exploding_registry();
    assert!(registry.load_dir(root.path())?.is_clean());

    let outcomes = registry.dispatch(|probe| probe.run(String::new()));
    let error = outcomes
        .first()
        .ok_or("one plugin was dispatched to")?
        .result
        .as_ref()
        .err()
        .ok_or("the call should have failed")?;
    assert!(
        panicked_in(error).is_none(),
        "a Lua error should not be reported as a panic: {error}"
    );
    assert!(error.to_string().contains("just a failure"), "got: {error}");
    Ok(())
}

#[cfg(feature = "async")]
#[tokio::test]
async fn a_panic_in_an_awaited_call_fails_the_call() -> TestResult {
    let root = tempfile::tempdir()?;
    write_plugin(
        root.path(),
        "boom()\n  return \"unreachable\"",
        "name = \"probe\"\n\n[capabilities]\nboom = {}\n",
    )?;

    let mut registry = exploding_registry();
    assert!(registry.load_dir(root.path())?.is_clean());

    let outcomes = registry
        .dispatch_async(|probe| async move { probe.run(String::new()) })
        .await;
    let error = outcomes
        .first()
        .ok_or("one plugin was dispatched to")?
        .result
        .as_ref()
        .err()
        .ok_or("the call should have failed")?;
    assert!(
        panicked_in(error).is_some(),
        "expected a panic report: {error}"
    );
    Ok(())
}

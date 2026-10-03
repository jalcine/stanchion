//! A Rust panic raised under a plugin call is reported, not propagated.
//!
//! These tests deliberately panic, so each one prints a panic message and backtrace to
//! stderr on the way through. That output is the panic being handled, not a failure.
#![cfg(feature = "registry")]

mod common;

use std::fs;

use common::TestResult;
use stanchion::registry::{CapabilityCall, FailureReason, Registry, Rules, Value};
use stanchion_lua::backend::LuaBackend;
use stanchion_lua::sandbox::Sandbox;

/// A host whose one capability panics when a plugin calls it.
#[expect(
    clippy::panic,
    reason = "the panic is the subject under test, not a fallible path taking a shortcut"
)]
fn exploding_registry() -> Registry {
    Registry::new()
        .with_runtime(Box::new(LuaBackend::isolated(Sandbox::restricted())))
        .with_setup(|host| {
            host.capability("boom", |_call: &CapabilityCall| {
                panic!("the provider gave up")
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
             function P.new(config, deps) return setmetatable({{deps = deps}}, P) end\n\
             function P:run(input)\n  {body}\nend\nreturn P\n"
        ),
    )?;
    Ok(())
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
    let outcomes = registry.dispatch("run", &[Value::Str(String::new())]);
    let outcome = outcomes.first().ok_or("one plugin was dispatched to")?;
    let error = outcome
        .error
        .as_ref()
        .ok_or("the call should have failed")?;

    assert!(
        error.contains("panicked") && error.contains("the provider gave up"),
        "not a panic report: {error}"
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
        matches!(&failure.reason, FailureReason::Runtime(reason) if reason.contains("panicked")),
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

    let outcomes = registry.dispatch("run", &[Value::Str(String::new())]);
    let error = outcomes
        .first()
        .ok_or("one plugin was dispatched to")?
        .error
        .as_ref()
        .ok_or("the call should have failed")?;
    assert!(
        !error.contains("panicked"),
        "a Lua error should not be reported as a panic: {error}"
    );
    assert!(error.contains("just a failure"), "got: {error}");
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

    let outcomes = registry.dispatch_async("run", &[]).await;
    let error = outcomes
        .first()
        .ok_or("one plugin was dispatched to")?
        .error
        .as_ref()
        .ok_or("the call should have failed")?;
    assert!(
        error.contains("panicked"),
        "expected a panic report: {error}"
    );
    Ok(())
}

/// A third-party backend that panics in `call` must not unwind into the caller.
///
/// The workspace's panic policy says every fallible path returns an error instead of
/// unwinding, and `Registry::dispatch` and `Registry::call_async` both wrapped their
/// calls in `panics::guard`. `Registry::call` did not — it was safe only because the
/// *Lua* backend happens to guard inside its own `PluginInstance::call`. Since
/// `PluginBackend` is public API, that put the guarantee on the wrong side of the
/// trait boundary: any backend a host writes itself would unwind straight through.
mod foreign_backend {
    use std::path::Path;

    use stanchion::abi::backend::{PluginBackend, PluginInstance};
    use stanchion::abi::load::{GroupOutcome, LoadContext, LoadItem};
    use stanchion::abi::manifest::{Manifest, PluginType};
    use stanchion::abi::runtime::{Reloaded, Runtime};
    use stanchion::abi::{Result as AbiResult, Value};

    /// Panics on every call, the way a buggy third-party backend would.
    pub struct Exploding;

    /// Records whether the host asked it to drop a capability.
    pub struct Revocable(pub std::sync::Mutex<Vec<String>>);

    impl PluginInstance for Revocable {
        fn call(&self, _method: &str, _args: &[Value]) -> AbiResult<Value> {
            Ok(Value::Str("ok".to_string()))
        }

        fn runtime(&self) -> &str {
            "revocable"
        }

        fn revoke_capability(&self, capability: &str) -> bool {
            match self.0.lock() {
                Ok(mut seen) => {
                    seen.push(capability.to_string());
                    true
                }
                Err(_) => false,
            }
        }
    }

    impl PluginInstance for Exploding {
        #[expect(
            clippy::panic,
            reason = "the panic is the subject under test"
        )]
        fn call(&self, _method: &str, _args: &[Value]) -> AbiResult<Value> {
            panic!("the backend gave up")
        }

        fn runtime(&self) -> &str {
            "exploding"
        }
    }

    impl PluginBackend for Exploding {
        fn plugin_type(&self) -> PluginType {
            PluginType::Lua
        }

        fn load(&self, _m: &Manifest, _d: &Path) -> AbiResult<Box<dyn PluginInstance>> {
            Ok(Box::new(Exploding))
        }
    }

    impl Runtime for Exploding {
        fn runtime_name(&self) -> &'static str {
            "exploding"
        }

        fn load_group(&self, items: &[LoadItem], _ctx: &LoadContext) -> Vec<GroupOutcome> {
            items
                .iter()
                .map(|item| GroupOutcome::Loaded {
                    name: item.manifest.name.clone(),
                    instance: Box::new(Exploding),
                    granted: Vec::new(),
                })
                .collect()
        }

        fn reload_plugin(&self, _item: &LoadItem, _ctx: &LoadContext) -> AbiResult<Reloaded> {
            Ok(Reloaded {
                instance: Box::new(Exploding),
                granted: Vec::new(),
            })
        }

        fn unload(&self, _name: &str) {}
    }
}

#[test]
fn a_panicking_backend_fails_the_call_not_the_caller() -> TestResult {
    let root = tempfile::tempdir()?;
    let dir = root.path().join("probe");
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("plugin.toml"), "name = \"probe\"\n")?;
    fs::write(dir.join("init.lua"), "return {}\n")?;

    let mut registry = Registry::new().with_runtime(Box::new(foreign_backend::Exploding));
    registry.load_dir(root.path())?;

    let Err(error) = registry.call("probe", "run", &[Value::Str(String::new())]) else {
        return Err("a panicking backend should surface as an error".into());
    };
    assert!(
        error.to_string().contains("panic"),
        "expected the panic to be reported, got: {error}"
    );
    Ok(())
}

/// A non-Lua backend's instance must be able to honour revocation.
///
/// `Runtime::revoke_capability` took a `&dyn PluginInstance` and each backend
/// downcast it back to its own concrete type through `PluginInstance::as_any`. For the
/// Lua backend that worked; for anything else the downcast failed and revocation
/// quietly returned `false` while the registry still struck the capability off the
/// plugin's granted list. Revoking is a security operation, so "silently did nothing"
/// is the wrong failure. Asking the instance directly removes both the downcast and
/// the whole `as_any` escape hatch.
#[test]
fn revocation_reaches_a_foreign_backends_instance() -> TestResult {
    use stanchion::abi::backend::PluginInstance;

    let instance = foreign_backend::Revocable(std::sync::Mutex::new(Vec::new()));
    assert!(
        PluginInstance::revoke_capability(&instance, "kv"),
        "the instance should report that it held the capability"
    );
    let seen = instance.0.lock().map_err(|_| "poisoned")?.clone();
    assert_eq!(seen, vec!["kv".to_string()]);
    Ok(())
}

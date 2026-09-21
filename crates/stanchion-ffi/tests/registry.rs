//! The dynamic surface, driven the way a binding drives it.
//!
//! These mirror `stanchion/tests/remote.rs` deliberately. The two transports are
//! supposed to behave alike, so they are asserted against the same plugins and the
//! same expectations; a divergence here is a bug in one of them rather than a
//! difference worth keeping.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use stanchion_ffi::{
    CapabilityCall, CapabilityProvider, CapabilityRequest, Decision, Error, HostConfig, Policy,
    Stanchion, Value,
};
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A plugin whose methods exercise the value boundary and the failure paths.
const ECHO: &str = r#"
local P = {}
P.__index = P

function P.new(config)
  return setmetatable({ tag = config.tag or "echo" }, P)
end

function P:identity(value) return value end
function P:tagged() return self.tag end
function P:sum(a, b) return a + b end
function P:table() return { n = 1, xs = {"a", "b"} } end
function P:boom() error("deliberate") end

return P
"#;

fn write_plugin(root: &Path, name: &str, manifest: &str, source: &str) -> TestResult {
    let dir = root.join(name);
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("plugin.toml"), manifest)?;
    fs::write(dir.join("init.lua"), source)?;
    Ok(())
}

/// A registry over a throwaway root holding one `echo` plugin.
fn echo_host() -> Result<(TempDir, Stanchion), Box<dyn std::error::Error>> {
    let root = TempDir::new()?;
    write_plugin(
        root.path(),
        "echo",
        "name = \"echo\"\nentry = \"init.lua\"\n\n[config]\ntag = \"first\"\n",
        ECHO,
    )?;
    let host = Stanchion::builder().build()?;
    let report = host.load(Some(root.path()))?;
    assert!(report.is_clean(), "{:?}", report.failures);
    Ok((root, host))
}

#[test]
fn a_plugin_loads_and_answers() -> TestResult {
    let (_root, host) = echo_host()?;

    assert_eq!(host.names()?, vec!["echo".to_string()]);
    assert_eq!(host.len()?, 1);
    assert_eq!(
        host.call("echo", "tagged", &[])?,
        Value::Str("first".to_string())
    );
    assert_eq!(
        host.call("echo", "sum", &[Value::Int(2), Value::Int(3)])?,
        Value::Int(5)
    );
    Ok(())
}

#[test]
fn values_survive_the_boundary_in_both_directions() -> TestResult {
    let (_root, host) = echo_host()?;

    for value in [
        Value::Nil,
        Value::Bool(true),
        Value::Int(-9),
        Value::Float(1.5),
        Value::Str("hello".to_string()),
        Value::List(vec![Value::Int(1), Value::Int(2)]),
        Value::Map(BTreeMap::from([("k".to_string(), Value::Bool(false))])),
    ] {
        assert_eq!(
            host.call("echo", "identity", std::slice::from_ref(&value))?,
            value
        );
    }

    let Value::Map(table) = host.call("echo", "table", &[])? else {
        panic!("expected a map");
    };
    assert_eq!(table.get("n"), Some(&Value::Int(1)));
    Ok(())
}

#[test]
fn an_unknown_plugin_is_named_as_such() -> TestResult {
    let (_root, host) = echo_host()?;
    assert_eq!(
        host.call("absent", "identity", &[]).unwrap_err(),
        Error::UnknownPlugin("absent".to_string())
    );
    Ok(())
}

#[test]
fn a_raising_method_reports_rather_than_unwinding() -> TestResult {
    let (_root, host) = echo_host()?;
    let err = host.call("echo", "boom", &[]).unwrap_err();
    assert_eq!(err.kind(), "lua");
    assert!(err.to_string().contains("deliberate"), "{err}");
    Ok(())
}

#[test]
fn one_bad_plugin_does_not_stop_the_others() -> TestResult {
    let root = TempDir::new()?;
    write_plugin(
        root.path(),
        "good",
        "name = \"good\"\nentry = \"init.lua\"\n",
        ECHO,
    )?;
    write_plugin(
        root.path(),
        "bad",
        "name = \"bad\"\nentry = \"init.lua\"\n",
        "this is not lua at all (((",
    )?;

    let host = Stanchion::builder().build()?;
    let report = host.load(Some(root.path()))?;

    assert_eq!(report.loaded, vec!["good".to_string()]);
    assert_eq!(report.failures.len(), 1);
    assert_eq!(report.failures[0].plugin, "bad");
    assert!(!report.is_clean());
    Ok(())
}

#[test]
fn a_dispatch_collects_every_plugins_answer() -> TestResult {
    let root = TempDir::new()?;
    for (name, tag) in [("a", "alpha"), ("b", "beta")] {
        write_plugin(
            root.path(),
            name,
            &format!("name = \"{name}\"\nentry = \"init.lua\"\n\n[config]\ntag = \"{tag}\"\n"),
            ECHO,
        )?;
    }

    let host = Stanchion::builder().build()?;
    host.load(Some(root.path()))?;

    let mut tags: Vec<String> = host
        .dispatch("tagged", &[])?
        .into_iter()
        .filter_map(|outcome| match outcome.value {
            Some(Value::Str(tag)) => Some(tag),
            _ => None,
        })
        .collect();
    tags.sort();
    assert_eq!(tags, vec!["alpha".to_string(), "beta".to_string()]);

    // A failing method is reported in place, not raised.
    let outcomes = host.dispatch("boom", &[])?;
    assert_eq!(outcomes.len(), 2);
    assert!(outcomes.iter().all(|outcome| outcome.error.is_some()));
    Ok(())
}

#[test]
fn a_reload_picks_up_the_new_source() -> TestResult {
    let root = TempDir::new()?;
    write_plugin(
        root.path(),
        "echo",
        "name = \"echo\"\nentry = \"init.lua\"\n\n[config]\ntag = \"before\"\n",
        ECHO,
    )?;

    let host = Stanchion::builder().build()?;
    host.load(Some(root.path()))?;
    assert_eq!(
        host.call("echo", "tagged", &[])?,
        Value::Str("before".to_string())
    );

    fs::write(
        root.path().join("echo/plugin.toml"),
        "name = \"echo\"\nentry = \"init.lua\"\n\n[config]\ntag = \"after\"\n",
    )?;
    host.reload("echo")?;
    assert_eq!(
        host.call("echo", "tagged", &[])?,
        Value::Str("after".to_string())
    );
    Ok(())
}

// ---- capabilities ----------------------------------------------------------

const CALLER: &str = r#"
local P = {}
P.__index = P
function P.new(_) return setmetatable({}, P) end
function P:shout(word) return greet(word) end
function P:attempt(word)
  local ok, err = pcall(function() return greet(word) end)
  return ok and err or ("refused: " .. tostring(err))
end
return P
"#;

const CALLER_MANIFEST: &str = "name = \"caller\"\nentry = \"init.lua\"\n\n[capabilities.greet]\n";

/// Records what it was asked, and answers.
struct Greeter {
    seen: Mutex<Vec<CapabilityCall>>,
    answer: Result<Value, String>,
}

impl CapabilityProvider for Greeter {
    fn invoke(&self, call: &CapabilityCall) -> Result<Value, String> {
        if let Ok(mut seen) = self.seen.lock() {
            seen.push(call.clone());
        }
        self.answer.clone()
    }
}

#[test]
fn a_plugin_reaches_a_foreign_capability() -> TestResult {
    let root = TempDir::new()?;
    write_plugin(root.path(), "caller", CALLER_MANIFEST, CALLER)?;

    let greeter = Arc::new(Greeter {
        seen: Mutex::new(Vec::new()),
        answer: Ok(Value::Str("hi there".to_string())),
    });
    let host = Stanchion::builder()
        .capability("greet", Arc::clone(&greeter) as Arc<dyn CapabilityProvider>)
        .build()?;

    let report = host.load(Some(root.path()))?;
    assert!(report.is_clean(), "{:?}", report.failures);

    assert_eq!(
        host.call("caller", "shout", &[Value::Str("world".to_string())])?,
        Value::Str("hi there".to_string())
    );

    let seen = greeter.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].plugin, "caller");
    assert_eq!(seen[0].capability, "greet");
    assert_eq!(seen[0].args, vec![Value::Str("world".to_string())]);
    Ok(())
}

#[test]
fn a_refusing_provider_is_catchable_inside_lua() -> TestResult {
    let root = TempDir::new()?;
    write_plugin(root.path(), "caller", CALLER_MANIFEST, CALLER)?;

    let host = Stanchion::builder()
        .capability(
            "greet",
            Arc::new(Greeter {
                seen: Mutex::new(Vec::new()),
                answer: Err("not today".to_string()),
            }) as Arc<dyn CapabilityProvider>,
        )
        .build()?;
    host.load(Some(root.path()))?;

    let Value::Str(message) = host.call("caller", "attempt", &[Value::Str("x".to_string())])?
    else {
        panic!("expected a string");
    };
    assert!(message.contains("refused"), "{message}");
    assert!(message.contains("not today"), "{message}");
    Ok(())
}

#[test]
fn a_capability_nobody_provides_keeps_the_plugin_out() -> TestResult {
    let root = TempDir::new()?;
    write_plugin(root.path(), "caller", CALLER_MANIFEST, CALLER)?;

    // No provider, no allow-list: the default policy denies.
    let host = Stanchion::builder().build()?;
    let report = host.load(Some(root.path()))?;

    assert!(report.loaded.is_empty());
    assert_eq!(report.failures.len(), 1);
    Ok(())
}

#[test]
fn a_policy_can_narrow_what_a_plugin_asked_for() -> TestResult {
    let root = TempDir::new()?;
    write_plugin(
        root.path(),
        "caller",
        "name = \"caller\"\nentry = \"init.lua\"\n\n[capabilities.greet]\nhosts = [\"*\"]\n",
        CALLER,
    )?;

    let narrowing = |request: &CapabilityRequest| -> Decision {
        assert_eq!(request.capability, "greet");
        Decision::GrantWith(Value::Map(BTreeMap::from([(
            "hosts".to_string(),
            Value::List(vec![Value::Str("example.com".to_string())]),
        )])))
    };

    let greeter = Arc::new(Greeter {
        seen: Mutex::new(Vec::new()),
        answer: Ok(Value::Nil),
    });
    let host = Stanchion::builder()
        .capability("greet", Arc::clone(&greeter) as Arc<dyn CapabilityProvider>)
        .policy(Arc::new(narrowing) as Arc<dyn Policy>)
        .build()?;
    host.load(Some(root.path()))?;
    host.call("caller", "shout", &[Value::Str("x".to_string())])?;

    // The provider must see what policy approved, not what the manifest asked for.
    let seen = greeter.seen.lock().unwrap();
    let Value::Map(grant) = &seen[0].grant else {
        panic!("expected a map");
    };
    assert_eq!(
        grant.get("hosts"),
        Some(&Value::List(vec![Value::Str("example.com".to_string())]))
    );
    Ok(())
}

#[test]
fn revoking_a_capability_defangs_a_live_plugin() -> TestResult {
    let root = TempDir::new()?;
    write_plugin(root.path(), "caller", CALLER_MANIFEST, CALLER)?;

    let host = Stanchion::builder()
        .capability(
            "greet",
            Arc::new(Greeter {
                seen: Mutex::new(Vec::new()),
                answer: Ok(Value::Str("ok".to_string())),
            }) as Arc<dyn CapabilityProvider>,
        )
        .build()?;
    host.load(Some(root.path()))?;

    assert!(host.call("caller", "shout", &[Value::Nil]).is_ok());
    assert!(host.revoke("caller", "greet")?);
    assert!(host.call("caller", "shout", &[Value::Nil]).is_err());
    assert!(!host.revoke("caller", "greet")?, "already gone");
    Ok(())
}

/// Calls back into the registry that invoked it — the deadlock this must not be.
struct Reenters {
    host: Mutex<Option<Arc<Stanchion>>>,
}

impl CapabilityProvider for Reenters {
    fn invoke(&self, _call: &CapabilityCall) -> Result<Value, String> {
        let host = self.host.lock().map_err(|_| "poisoned".to_string())?;
        let host = host.as_ref().ok_or("no host")?;
        match host.call("caller", "shout", &[Value::Nil]) {
            Err(err) => Err(err.to_string()),
            Ok(_) => Err("the re-entry should not have succeeded".to_string()),
        }
    }
}

#[test]
fn a_provider_that_re_enters_gets_an_error_rather_than_a_hang() -> TestResult {
    let root = TempDir::new()?;
    write_plugin(root.path(), "caller", CALLER_MANIFEST, CALLER)?;

    let provider = Arc::new(Reenters {
        host: Mutex::new(None),
    });
    let host = Arc::new(
        Stanchion::builder()
            .capability("greet", Arc::clone(&provider) as Arc<dyn CapabilityProvider>)
            .build()?,
    );
    *provider.host.lock().unwrap() = Some(Arc::clone(&host));

    host.load(Some(root.path()))?;

    // Without the guard this call never returns.
    let err = host
        .call("caller", "shout", &[Value::Nil])
        .expect_err("re-entry must fail");
    assert!(
        err.to_string().contains("deadlock"),
        "expected the reentrancy error, got: {err}"
    );
    Ok(())
}

// ---- configuration ---------------------------------------------------------

#[test]
fn an_unknown_standard_library_is_a_configuration_error() {
    let mut config = HostConfig::default();
    config.sandbox.libs = Some(vec!["sorcery".to_string()]);

    let err = Stanchion::builder()
        .config(config)
        .build()
        .expect_err("an unknown library must not build");
    assert_eq!(err.kind(), "config");
}

#[test]
fn loading_without_a_root_says_so() -> TestResult {
    let host = Stanchion::builder().build()?;
    assert_eq!(host.load(None).unwrap_err().kind(), "config");
    Ok(())
}

#[test]
fn a_configured_root_is_used_when_none_is_passed() -> TestResult {
    let root = TempDir::new()?;
    write_plugin(
        root.path(),
        "echo",
        "name = \"echo\"\nentry = \"init.lua\"\n",
        ECHO,
    )?;

    let config = HostConfig {
        plugins: Some(root.path().to_path_buf()),
        ..HostConfig::default()
    };
    let host = Stanchion::builder().config(config).build()?;
    assert!(host.load(None)?.is_clean());
    assert_eq!(host.names()?, vec!["echo".to_string()]);
    Ok(())
}

#[test]
fn an_audit_reports_requests_without_running_the_plugin() -> TestResult {
    let root = TempDir::new()?;
    write_plugin(
        root.path(),
        "caller",
        CALLER_MANIFEST,
        "error('this plugin must never be executed')",
    )?;

    let host = Stanchion::builder().build()?;
    let audit = host.audit(Some(root.path()))?;

    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].plugin, "caller");
    assert_eq!(audit[0].capabilities, vec!["greet".to_string()]);
    Ok(())
}

#[test]
fn isolation_is_per_plugin_unless_asked_otherwise() -> TestResult {
    assert_eq!(Stanchion::builder().build()?.isolation()?, "per-plugin");

    let mut config = HostConfig::default();
    config.sandbox.shared = true;
    assert_eq!(
        Stanchion::builder().config(config).build()?.isolation()?,
        "shared"
    );
    Ok(())
}

// ---- the builder's configuration reaches the plugins -----------------------
//
// `Builder::build()` once constructed two registries: a configured one handed to the
// Lua backend, and a bare `Registry::new(Lua::new())` stored as the field every other
// method locks. Nothing the builder set up survived the call (#2).
//
// Eight tests in this workspace failed under that bug, but all of them incidentally —
// they wanted a capability bound and a bare registry binds none. None of them named
// the security guarantee that was actually lost. These two do, so a reintroduction
// fails on the property rather than on a symptom.

/// Reports which standard libraries its state can actually see.
const PROBER: &str = r#"
local P = {}
P.__index = P

function P.new(_) return setmetatable({}, P) end

function P:sees(name) return _G[name] ~= nil end

return P
"#;

#[test]
fn the_configured_sandbox_reaches_the_plugins_that_load() -> TestResult {
    let root = TempDir::new()?;
    write_plugin(
        root.path(),
        "prober",
        "name = \"prober\"\nentry = \"init.lua\"\n",
        PROBER,
    )?;

    // The default sandbox is `restricted()`: no `io`, no `os`, no `debug`.
    let host = Stanchion::builder().build()?;
    assert!(host.load(Some(root.path()))?.is_clean());

    for library in ["io", "os", "debug"] {
        assert_eq!(
            host.call("prober", "sees", &[Value::Str(library.to_string())])?,
            Value::Bool(false),
            "`{library}` must not be reachable from a plugin under the default sandbox",
        );
    }

    // A library the restricted set does keep, so the assertion above is discriminating
    // rather than a plugin that simply cannot see anything.
    assert_eq!(
        host.call("prober", "sees", &[Value::Str("string".to_string())])?,
        Value::Bool(true)
    );
    Ok(())
}

#[cfg(feature = "signatures")]
#[test]
fn requiring_signatures_refuses_an_unsigned_plugin() -> TestResult {
    let root = TempDir::new()?;
    write_plugin(
        root.path(),
        "echo",
        "name = \"echo\"\nentry = \"init.lua\"\n",
        ECHO,
    )?;

    let config = HostConfig {
        signatures: stanchion_ffi::SignatureConfig { required: true },
        ..HostConfig::default()
    };
    let host = Stanchion::builder().config(config).build()?;
    let report = host.load(Some(root.path()))?;

    assert!(
        report.loaded.is_empty(),
        "an unsigned plugin loaded under `required = true`: {:?}",
        report.loaded
    );
    assert_eq!(report.failures.len(), 1);
    assert_eq!(report.failures[0].plugin, "echo");

    // Refused, not merely unreported: the plugin must not be callable either.
    assert_eq!(host.len()?, 0);
    assert_eq!(
        host.call("echo", "tagged", &[]).unwrap_err(),
        Error::UnknownPlugin("echo".to_string())
    );
    Ok(())
}

#[cfg(feature = "signatures")]
#[test]
fn signatures_stay_optional_unless_the_host_asks() -> TestResult {
    // The companion to the test above: the refusal has to come from the configuration,
    // not from the plugin being unloadable for some unrelated reason.
    let root = TempDir::new()?;
    write_plugin(
        root.path(),
        "echo",
        "name = \"echo\"\nentry = \"init.lua\"\n",
        ECHO,
    )?;

    let host = Stanchion::builder().build()?;
    assert!(host.load(Some(root.path()))?.is_clean());
    assert_eq!(host.len()?, 1);
    Ok(())
}

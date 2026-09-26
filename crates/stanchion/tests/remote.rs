//! The out-of-process host, driven end to end against the real binary.
#![cfg(feature = "remote")]

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value as Json, json};
use stanchion::remote::{CallbackCall, RemoteOptions, RemoteRegistry};
use tempfile::TempDir;

use common::write_plugin;

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;
type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Cargo builds the host binary for this package's tests and hands over its path,
/// so there is no pre-build step and no guessing where it landed.
fn host_binary() -> Fallible<PathBuf> {
    Ok(PathBuf::from(env!("CARGO_BIN_EXE_plugin-host")))
}

/// A plugin whose methods exercise the JSON boundary and the failure paths.
const ECHO: &str = r#"
local P = {}
P.__index = P

function P.new(config, deps)
  return setmetatable({ prefix = config.prefix or "" }, P)
end

function P:greet(who)
  return self.prefix .. "hello, " .. who
end

function P:add(a, b)
  return a + b
end

function P:roundtrip(value)
  return value
end

function P:explode()
  error("plugin said no")
end

function P:spin()
  while true do end
end

function P:hog()
  local t = {}
  for i = 1, 1e9 do t[i] = string.rep("x", 1024) end
  return "never"
end

return P
"#;

fn plugin_root() -> Fallible<TempDir> {
    let root = tempfile::tempdir()?;
    write_plugin(
        root.path(),
        "echo",
        "name = \"echo\"\nversion = \"1.0.0\"\n\n[config]\nprefix = \"> \"\n",
        ECHO,
    )?;
    Ok(root)
}

fn launch(root: &TempDir, config: Option<&Path>) -> Fallible<RemoteRegistry> {
    let mut options = RemoteOptions::new(host_binary()?).plugins(root.path());
    if let Some(config) = config {
        options = options.config(config);
    }
    Ok(RemoteRegistry::launch(options)?)
}

#[test]
fn calls_a_plugin_in_another_process() -> TestResult {
    let root = plugin_root()?;
    let mut remote = launch(&root, None)?;

    let plugins = remote.list()?;
    assert_eq!(plugins.len(), 1);
    let echo = plugins.first().ok_or("expected one plugin")?;
    assert_eq!(echo.name, "echo");
    assert_eq!(echo.version.as_deref(), Some("1.0.0"));

    let greeting: String = remote.call("echo", "greet", [json!("world")])?;
    assert_eq!(greeting, "> hello, world", "config reached the child");

    let sum: i64 = remote.call("echo", "add", [json!(2), json!(40)])?;
    assert_eq!(sum, 42);

    remote.shutdown()?;
    Ok(())
}

#[test]
fn json_survives_the_round_trip() -> TestResult {
    let root = plugin_root()?;
    let mut remote = launch(&root, None)?;

    // Lua tables are the lossy part of this boundary; check the shapes that matter.
    let list: Vec<i64> = remote.call("echo", "roundtrip", [json!([1, 2, 3])])?;
    assert_eq!(list, [1, 2, 3]);

    let object: Json = remote.call("echo", "roundtrip", [json!({"a": 1, "b": "two"})])?;
    assert_eq!(object.get("a"), Some(&json!(1)));
    assert_eq!(object.get("b"), Some(&json!("two")));

    let text: String = remote.call("echo", "roundtrip", [json!("plain")])?;
    assert_eq!(text, "plain");

    let flag: bool = remote.call("echo", "roundtrip", [json!(true)])?;
    assert!(flag);

    remote.shutdown()?;
    Ok(())
}

#[test]
fn a_plugin_error_does_not_kill_the_host() -> TestResult {
    let root = plugin_root()?;
    let mut remote = launch(&root, None)?;

    let Err(error) = remote.call::<Json>("echo", "explode", []) else {
        return Err("the method raises, so the call must fail".into());
    };
    let error = error.to_string();
    assert!(error.contains("plugin said no"), "got: {error}");

    // The whole point: the host survives and keeps serving.
    assert!(remote.is_alive());
    let greeting: String = remote.call("echo", "greet", [json!("again")])?;
    assert_eq!(greeting, "> hello, again");

    remote.shutdown()?;
    Ok(())
}

#[test]
fn an_unknown_method_is_an_error_not_a_crash() -> TestResult {
    let root = plugin_root()?;
    let mut remote = launch(&root, None)?;

    let failed = remote.call::<Json>("echo", "nonexistent", []);
    assert!(failed.is_err());
    assert!(remote.is_alive());

    let Err(error) = remote.call::<Json>("nosuchplugin", "greet", []) else {
        return Err("an unknown plugin must fail".into());
    };
    let error = error.to_string();
    assert!(error.contains("no plugin named"), "got: {error}");

    remote.shutdown()?;
    Ok(())
}

#[test]
fn a_runaway_loop_is_stopped_by_the_hosts_instruction_limit() -> TestResult {
    let root = plugin_root()?;
    let config = root.path().join("host.toml");
    fs::write(
        &config,
        "[sandbox]\ninstruction_limit = 200000\nmemory_limit = 33554432\n",
    )?;
    let mut remote = launch(&root, Some(&config))?;

    // In-process this would hang the application; here it is a failed call.
    let Err(error) = remote.call::<Json>("echo", "spin", []) else {
        return Err("an endless loop must hit the limit".into());
    };
    let error = error.to_string();
    assert!(error.contains("instruction limit"), "got: {error}");

    assert!(remote.is_alive());
    remote.shutdown()?;
    Ok(())
}

#[test]
fn an_allocation_storm_is_stopped_by_the_hosts_memory_limit() -> TestResult {
    let root = plugin_root()?;
    let config = root.path().join("host.toml");
    fs::write(&config, "[sandbox]\nmemory_limit = 8388608\n")?;
    let mut remote = launch(&root, Some(&config))?;

    let Err(error) = remote.call::<Json>("echo", "hog", []) else {
        return Err("an allocation storm must hit the limit".into());
    };
    let error = error.to_string();
    assert!(
        error.to_lowercase().contains("memory") || error.contains("instruction"),
        "got: {error}"
    );

    assert!(remote.is_alive());
    remote.shutdown()?;
    Ok(())
}

#[test]
fn dispatch_reports_one_outcome_per_plugin() -> TestResult {
    let root = tempfile::tempdir()?;
    write_plugin(root.path(), "good", "name = \"good\"\n", ECHO)?;
    write_plugin(
        root.path(),
        "bad",
        "name = \"bad\"\n",
        "local P = {}\nP.__index = P\n\
         function P.new() return setmetatable({}, P) end\n\
         function P:greet(who) error(\"bad refuses\") end\n\
         return P\n",
    )?;
    let mut remote = launch(&root, None)?;

    let outcomes = remote.dispatch("greet", [json!("world")])?;
    assert_eq!(outcomes.len(), 2);

    let find = |name: &str| {
        outcomes
            .iter()
            .find(|outcome| outcome.plugin == name)
            .cloned()
    };
    let good = find("good").ok_or("expected an outcome for good")?;
    assert_eq!(good.value, Some(json!("hello, world")));
    assert!(good.error.is_none());

    let bad = find("bad").ok_or("expected an outcome for bad")?;
    assert!(bad.value.is_none());
    assert!(
        bad.error
            .as_deref()
            .is_some_and(|error| error.contains("bad refuses")),
        "got: {:?}",
        bad.error
    );

    remote.shutdown()?;
    Ok(())
}

#[test]
fn a_broken_plugin_is_reported_and_the_rest_load() -> TestResult {
    let root = tempfile::tempdir()?;
    write_plugin(root.path(), "echo", "name = \"echo\"\n", ECHO)?;
    write_plugin(
        root.path(),
        "broken",
        "name = \"broken\"\n",
        "error('boom')\n",
    )?;

    let mut remote =
        RemoteRegistry::launch(RemoteOptions::new(host_binary()?).inherit_stderr(false))?;

    let outcome = remote.load(root.path())?;
    assert!(!outcome.is_clean());
    assert!(outcome.loaded.contains(&"echo".to_string()));
    assert!(
        outcome
            .failures
            .iter()
            .any(|failure| failure.plugin == "broken"),
        "got: {:?}",
        outcome.failures
    );

    remote.shutdown()?;
    Ok(())
}

#[test]
fn reload_takes_effect_in_the_child() -> TestResult {
    let root = plugin_root()?;
    let mut remote = launch(&root, None)?;

    let greeting: String = remote.call("echo", "greet", [json!("world")])?;
    assert_eq!(greeting, "> hello, world");

    fs::write(
        root.path().join("echo").join("plugin.toml"),
        "name = \"echo\"\nversion = \"2.0.0\"\n\n[config]\nprefix = \"! \"\n",
    )?;
    remote.reload("echo")?;

    let greeting: String = remote.call("echo", "greet", [json!("world")])?;
    assert_eq!(greeting, "! hello, world");
    remote.shutdown()?;
    Ok(())
}

#[test]
fn audit_runs_in_the_child_without_executing_plugins() -> TestResult {
    let root = tempfile::tempdir()?;
    write_plugin(
        root.path(),
        "greedy",
        "name = \"greedy\"\n\n[capabilities.log]\n",
        "error('audit must not execute plugin code')\n",
    )?;

    // No --plugins: the host serves without loading anything, so a successful audit
    // proves the report came from manifests alone.
    let mut remote =
        RemoteRegistry::launch(RemoteOptions::new(host_binary()?).inherit_stderr(false))?;
    let entries = remote.audit(root.path())?;
    assert_eq!(entries.len(), 1);
    let entry = entries.first().ok_or("expected one audit entry")?;
    assert_eq!(entry.plugin, "greedy");
    assert_eq!(entry.capabilities, ["log"]);

    remote.shutdown()?;
    Ok(())
}

#[test]
fn the_host_reports_its_own_configuration() -> TestResult {
    let root = plugin_root()?;
    let mut remote = launch(&root, None)?;

    // A separate process should not stop isolating at the process boundary.
    assert_eq!(remote.info()?.isolation, "per-plugin");
    remote.shutdown()?;
    Ok(())
}

#[test]
fn a_dropped_registry_does_not_leak_the_child() -> TestResult {
    let root = plugin_root()?;
    let mut remote = launch(&root, None)?;
    let greeting: String = remote.call("echo", "greet", [json!("world")])?;
    assert_eq!(greeting, "> hello, world");

    drop(remote);
    // Nothing to assert directly; Drop kills the child, and a leak would show up as
    // the test harness hanging on exit.
    Ok(())
}

#[test]
fn a_plugin_that_kills_its_process_does_not_take_the_application_with_it() -> TestResult {
    // The whole reason this module exists. `os.exit` is the bluntest version of what a
    // segfaulting C rock or a failed allocator would do; in-process there is no
    // recovering from it, and here it is one error value.
    let root = tempfile::tempdir()?;
    write_plugin(
        root.path(),
        "suicide",
        "name = \"suicide\"\n",
        "local P = {}\nP.__index = P\n\
         function P.new() return setmetatable({}, P) end\n\
         function P:die() os.exit(3) end\n\
         function P:ping() return \"alive\" end\n\
         return P\n",
    )?;

    let config = root.path().join("host.toml");
    fs::write(
        &config,
        "[sandbox]\nlibs = [\"string\", \"table\", \"math\", \"coroutine\", \"package\", \"os\"]\ndeny = []\n",
    )?;

    let mut remote = launch(&root, Some(&config))?;
    let alive: String = remote.call("suicide", "ping", [])?;
    assert_eq!(alive, "alive");

    let Err(error) = remote.call::<Json>("suicide", "die", []) else {
        return Err("the host process exits, so the call must fail".into());
    };
    let error = error.to_string();
    assert!(
        error.contains("plugin host exited"),
        "a dead host should be reported as such, got: {error}"
    );
    assert!(!remote.is_alive());

    // The test process itself is untouched, which is the point being asserted.
    Ok(())
}

// ---------------------------------------------------------------------------
// Callbacks: a plugin in the child reaching back into this process.
// ---------------------------------------------------------------------------

/// Declares a `kv` capability and uses it, so the plugin depends on the application.
const CALLER: &str = r#"
local P = {}
P.__index = P

function P.new(config, deps)
  return setmetatable({}, P)
end

function P:lookup(key)
  return kv(key)
end

function P:lookup_twice(a, b)
  return kv(a) .. "/" .. kv(b)
end

function P:failing()
  return kv("boom")
end

function P:undeclared()
  return type(kv)
end

return P
"#;

fn caller_root(manifest: &str) -> Fallible<TempDir> {
    let root = tempfile::tempdir()?;
    write_plugin(root.path(), "caller", manifest, CALLER)?;
    let config = root.path().join("host.toml");
    fs::write(&config, "[capabilities]\ncallbacks = [\"kv\"]\n")?;
    Ok(root)
}

#[test]
fn a_plugin_calls_back_into_the_application() -> TestResult {
    let root = caller_root("name = \"caller\"\n\n[capabilities.kv]\n")?;
    let config = root.path().join("host.toml");

    let options = RemoteOptions::new(host_binary()?)
        .config(&config)
        .plugins(root.path())
        .inherit_stderr(false);
    let mut remote = RemoteRegistry::launch(options)?.on_callback(|call: &CallbackCall| {
        assert_eq!(call.capability, "kv");
        assert_eq!(call.plugin, "caller");
        match call.args.first().and_then(Json::as_str) {
            Some("boom") => Err("no such key".to_string()),
            Some(key) => Ok(json!(format!("value-of-{key}"))),
            None => Err("kv takes one key".to_string()),
        }
    });

    let value: String = remote.call("caller", "lookup", [json!("alpha")])?;
    assert_eq!(value, "value-of-alpha");

    // Several callbacks inside one plugin call, all interleaved on one channel.
    let pair: String = remote.call("caller", "lookup_twice", [json!("a"), json!("b")])?;
    assert_eq!(pair, "value-of-a/value-of-b");

    remote.shutdown()?;
    Ok(())
}

#[test]
fn an_application_error_surfaces_as_a_lua_error() -> TestResult {
    let root = caller_root("name = \"caller\"\n\n[capabilities.kv]\n")?;
    let config = root.path().join("host.toml");

    let options = RemoteOptions::new(host_binary()?)
        .config(&config)
        .plugins(root.path())
        .inherit_stderr(false);
    let mut remote = RemoteRegistry::launch(options)?
        .on_callback(|_: &CallbackCall| Err("the application refused".to_string()));

    let Err(error) = remote.call::<Json>("caller", "failing", []) else {
        return Err("a refused callback must fail the plugin call".into());
    };
    assert!(
        error.to_string().contains("the application refused"),
        "got: {error}"
    );

    // The host and the application both survive a refused callback.
    assert!(remote.is_alive());
    remote.shutdown()?;
    Ok(())
}

#[test]
fn an_unhandled_callback_fails_without_killing_anything() -> TestResult {
    let root = caller_root("name = \"caller\"\n\n[capabilities.kv]\n")?;
    let config = root.path().join("host.toml");

    // No `on_callback` at all: the host offers `kv`, this application does not answer.
    let options = RemoteOptions::new(host_binary()?)
        .config(&config)
        .plugins(root.path())
        .inherit_stderr(false);
    let mut remote = RemoteRegistry::launch(options)?;

    let Err(error) = remote.call::<Json>("caller", "lookup", [json!("alpha")]) else {
        return Err("an unanswered callback must fail".into());
    };
    assert!(
        error.to_string().contains("does not handle"),
        "got: {error}"
    );
    assert!(remote.is_alive());
    remote.shutdown()?;
    Ok(())
}

#[test]
fn an_undeclared_capability_is_absent_even_when_the_host_offers_it() -> TestResult {
    // The host forwards `kv`, but this plugin's manifest never asks for it.
    let root = caller_root("name = \"caller\"\n")?;
    let config = root.path().join("host.toml");

    let options = RemoteOptions::new(host_binary()?)
        .config(&config)
        .plugins(root.path())
        .inherit_stderr(false);
    let mut remote = RemoteRegistry::launch(options)?
        .on_callback(|_: &CallbackCall| Ok(json!("should never be reached")));

    let kind: String = remote.call("caller", "undeclared", [])?;
    assert_eq!(
        kind, "nil",
        "capabilities stay declared-only across the process line"
    );
    remote.shutdown()?;
    Ok(())
}

#[test]
fn the_approved_grant_travels_with_every_callback() -> TestResult {
    let root = caller_root("name = \"caller\"\n\n[capabilities.kv]\nnamespace = \"tenant-7\"\n")?;
    let config = root.path().join("host.toml");

    let options = RemoteOptions::new(host_binary()?)
        .config(&config)
        .plugins(root.path())
        .inherit_stderr(false);
    let mut remote = RemoteRegistry::launch(options)?.on_callback(|call: &CallbackCall| {
        // The application re-checks rather than trusting the host's narrowing.
        let namespace = call
            .grant
            .get("namespace")
            .and_then(Json::as_str)
            .unwrap_or("none");
        Ok(json!(format!("{namespace}:ok")))
    });

    let value: String = remote.call("caller", "lookup", [json!("k")])?;
    assert_eq!(value, "tenant-7:ok");
    remote.shutdown()?;
    Ok(())
}

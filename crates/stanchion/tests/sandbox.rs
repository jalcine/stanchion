//! Per-plugin Lua states: standard library selection and resource limits.
#![cfg(feature = "registry")]

mod common;

use stanchion::registry::{FailureReason, Registry, Value};
use stanchion_lua::backend::LuaBackend;
use stanchion_lua::sandbox::Sandbox;
use tempfile::TempDir;

use common::{Fallible, TestResult, first_failure, probe_source, run, run_named, write_plugin};

fn isolated(sandbox: Sandbox) -> Registry {
    Registry::new().with_runtime(Box::new(LuaBackend::isolated(sandbox)))
}

fn grouped(sandbox: Sandbox) -> Registry {
    Registry::new().with_runtime(Box::new(LuaBackend::grouped(sandbox)))
}

fn single_plugin(body: &str) -> Fallible<TempDir> {
    let root = tempfile::tempdir()?;
    write_plugin(
        root.path(),
        "probe",
        "name = \"probe\"\n",
        &probe_source(body),
    )?;
    Ok(root)
}

#[test]
fn restricted_sandbox_withholds_io_and_os() -> TestResult {
    let root = single_plugin(r#"return type(os) .. "/" .. type(io) .. "/" .. type(string)"#)?;
    let mut registry = isolated(Sandbox::restricted());
    let report = registry.load_dir(root.path())?;
    assert!(report.is_clean(), "failures: {:?}", report.failures);

    // `os` and `io` are absent; `string` is present.
    assert_eq!(run(&registry, "")?, "nil/nil/table");
    Ok(())
}

#[test]
fn permissive_sandbox_grants_io_and_os() -> TestResult {
    let root = single_plugin(r#"return type(os) .. "/" .. type(io)"#)?;
    let mut registry = isolated(Sandbox::permissive());
    registry.load_dir(root.path())?;

    assert_eq!(run(&registry, "")?, "table/table");
    Ok(())
}

#[test]
fn deny_list_removes_reachable_escapes() -> TestResult {
    let root = single_plugin(r#"return type(dofile) .. "/" .. type(package.loadlib)"#)?;
    let mut registry = isolated(Sandbox::restricted());
    registry.load_dir(root.path())?;

    // `package` is loaded so `require` works, but `loadlib` would load any .so.
    assert_eq!(run(&registry, "")?, "nil/nil");
    Ok(())
}

#[test]
fn explicit_lib_selection_is_honoured() -> TestResult {
    let root = single_plugin(r#"return type(string) .. "/" .. type(math)"#)?;
    let mut registry = isolated(
        Sandbox::restricted().libs(stanchion_lua::mlua::StdLib::STRING | stanchion_lua::mlua::StdLib::TABLE),
    );
    registry.load_dir(root.path())?;

    assert_eq!(run(&registry, "")?, "table/nil");
    Ok(())
}

#[test]
fn each_plugin_gets_its_own_globals() -> TestResult {
    let root = tempfile::tempdir()?;
    write_plugin(
        root.path(),
        "writer",
        "name = \"writer\"\n",
        &probe_source(r#"rawset(_G, "shared_marker", "written"); return "ok""#),
    )?;
    write_plugin(
        root.path(),
        "reader",
        "name = \"reader\"\n",
        &probe_source(r#"return tostring(rawget(_G, "shared_marker"))"#),
    )?;

    let mut registry = isolated(Sandbox::restricted());
    registry.load_dir(root.path())?;

    run_named(&registry, "writer", "")?;

    // A raw global write escapes the per-plugin environment, but not the state.
    assert_eq!(run_named(&registry, "reader", "")?, "nil");
    Ok(())
}

#[test]
fn memory_limit_is_enforced_per_plugin() -> TestResult {
    let root = single_plugin(
        r#"local t = {} for i = 1, 1e7 do t[i] = string.rep("x", 256) end return 'never'"#,
    )?;
    let mut registry = isolated(Sandbox::restricted().memory_limit(8 * 1024 * 1024));
    registry.load_dir(root.path())?;

    let Err(error) = registry.call("probe", "run", &[Value::Str(String::new())]) else {
        return Err("an allocation storm should hit the memory limit".into());
    };
    assert!(
        error.to_string().to_lowercase().contains("memory"),
        "expected a memory error, got: {error}"
    );
    Ok(())
}

#[cfg(not(feature = "luau"))]
#[test]
fn instruction_limit_stops_a_runaway_loop() -> TestResult {
    let root = single_plugin(r#"while true do end"#)?;
    let mut registry = isolated(Sandbox::restricted().instruction_limit(100_000));
    registry.load_dir(root.path())?;

    let Err(error) = registry.call("probe", "run", &[Value::Str(String::new())]) else {
        return Err("an infinite loop should hit the instruction limit".into());
    };
    assert!(
        error.to_string().contains("instruction limit"),
        "expected an instruction-limit error, got: {error}"
    );
    Ok(())
}

#[cfg(not(feature = "luau"))]
#[test]
fn the_instruction_budget_resets_between_dispatches() -> TestResult {
    // Each call gets the full allowance, so repeated work does not accumulate.
    let root = single_plugin(r#"local n = 0 for i = 1, 20000 do n = n + i end return "done""#)?;
    let mut registry = isolated(Sandbox::restricted().instruction_limit(500_000));
    registry.load_dir(root.path())?;

    for round in 0..5 {
        let outcomes = registry.dispatch("run", &[Value::Str(String::new())]);
        let outcome = outcomes.first().ok_or("expected one outcome")?;
        assert_eq!(
            outcome.value,
            Some(Value::Str("done".to_string())),
            "round {round} should not inherit the previous round's budget"
        );
    }
    Ok(())
}

#[test]
fn ambient_setup_installs_values_into_every_state() -> TestResult {
    let root = single_plugin(r#"return host_greeting"#)?;
    let mut registry =
        isolated(Sandbox::restricted()).with_setup(|host| {
            host.ambient("host_greeting", Value::Str("from the host".to_string()));
            Ok(())
        });
    let report = registry.load_dir(root.path())?;
    assert!(report.is_clean(), "failures: {:?}", report.failures);

    assert_eq!(run(&registry, "")?, "from the host");
    Ok(())
}

#[test]
fn dependencies_cannot_cross_isolated_states() -> TestResult {
    let root = tempfile::tempdir()?;
    write_plugin(
        root.path(),
        "base",
        "name = \"base\"\n",
        &probe_source(r#"return "base""#),
    )?;
    write_plugin(
        root.path(),
        "user",
        "name = \"user\"\n\n[dependencies]\nbase = \"*\"\n",
        &probe_source(r#"return "user""#),
    )?;

    let mut registry = isolated(Sandbox::restricted());
    let report = registry.load_dir(root.path())?;

    assert_eq!(report.loaded, ["base"]);
    assert!(
        matches!(first_failure(&report)?, FailureReason::Runtime(reason) if reason.contains("per-plugin isolation")),
        "got: {:?}",
        report.failures
    );
    Ok(())
}

#[test]
fn shared_isolation_remains_available() -> TestResult {
    let backend = LuaBackend::shared();
    assert_eq!(backend.isolation_name(), "shared");
    Ok(())
}

// ---------------------------------------------------------------------------
// Grouped isolation: a dependency chain shares a state, everything else does not.
// ---------------------------------------------------------------------------

/// A plugin that publishes `exports` and can read a dependency's.
fn wired_source(exported: &str, reads: Option<&str>) -> String {
    let body = match reads {
        Some(dep) => format!(r#"return self.deps["{dep}"].value"#),
        None => format!(r#"return "{exported}""#),
    };
    format!(
        r#"
local P = {{}}
P.__index = P
function P.new(config, deps)
  return setmetatable({{ deps = deps, exports = {{ value = "{exported}" }} }}, P)
end
function P:run(input)
  {body}
end
return P
"#
    )
}

#[test]
fn a_dependency_chain_shares_one_grouped_state() -> TestResult {
    let root = tempfile::tempdir()?;
    write_plugin(
        root.path(),
        "base",
        "name = \"base\"\n",
        &wired_source("from base", None),
    )?;
    write_plugin(
        root.path(),
        "user",
        "name = \"user\"\n\n[dependencies]\nbase = \"*\"\n",
        &wired_source("from user", Some("base")),
    )?;

    let mut registry = grouped(Sandbox::restricted());
    let report = registry.load_dir(root.path())?;
    assert!(report.is_clean(), "got: {:?}", report.failures);

    // The export crossed, which is the whole point: under per-plugin isolation this
    // plugin cannot even load.
    assert_eq!(run_named(&registry, "user", "")?, "from base");
    assert_eq!(run_named(&registry, "base", "")?, "from base");
    Ok(())
}

#[test]
fn unrelated_plugins_get_separate_grouped_states() -> TestResult {
    let root = tempfile::tempdir()?;
    // Each writes a global and reads it back, so a shared state would show the other's.
    for name in ["left", "right"] {
        write_plugin(
            root.path(),
            name,
            &format!("name = \"{name}\"\n"),
            &probe_source(&format!(
                r#"marker = (marker or "") .. "{name}"
  return marker"#
            )),
        )?;
    }

    let mut registry = grouped(Sandbox::restricted());
    assert!(registry.load_dir(root.path())?.is_clean());

    // Neither sees the other's global, so the states really are separate.
    assert_eq!(run_named(&registry, "left", "")?, "left");
    assert_eq!(run_named(&registry, "right", "")?, "right");
    Ok(())
}

#[test]
fn a_diamond_of_dependencies_lands_in_one_group() -> TestResult {
    let root = tempfile::tempdir()?;
    // shared ← left, right ← top: undirected connectivity makes this one component,
    // which is what an incremental grouping would get wrong.
    write_plugin(
        root.path(),
        "shared",
        "name = \"shared\"\n",
        &wired_source("shared", None),
    )?;
    for name in ["left", "right"] {
        write_plugin(
            root.path(),
            name,
            &format!("name = \"{name}\"\n\n[dependencies]\nshared = \"*\"\n"),
            &wired_source(name, Some("shared")),
        )?;
    }
    write_plugin(
        root.path(),
        "top",
        "name = \"top\"\n\n[dependencies]\nleft = \"*\"\nright = \"*\"\n",
        &wired_source("top", Some("right")),
    )?;
    // An island, to prove the partition is not just "everything together".
    write_plugin(
        root.path(),
        "island",
        "name = \"island\"\n",
        &wired_source("island", None),
    )?;

    let mut registry = grouped(Sandbox::restricted());
    let report = registry.load_dir(root.path())?;
    assert!(report.is_clean(), "got: {:?}", report.failures);

    // The whole chain resolves through live proxies to the same surfaces.
    assert_eq!(run_named(&registry, "top", "")?, "right");
    assert_eq!(run_named(&registry, "left", "")?, "shared");
    assert_eq!(run_named(&registry, "right", "")?, "shared");
    assert_eq!(run_named(&registry, "island", "")?, "island");
    Ok(())
}

#[test]
fn a_group_shares_one_instruction_budget() -> TestResult {
    let root = tempfile::tempdir()?;
    // `user` burns instructions; `base` and the island only report.
    write_plugin(
        root.path(),
        "base",
        "name = \"base\"\n",
        &wired_source("base", None),
    )?;
    write_plugin(
        root.path(),
        "user",
        "name = \"user\"\n\n[dependencies]\nbase = \"*\"\n",
        &probe_source("for _ = 1, 200000 do end\n  return \"burned\""),
    )?;
    write_plugin(
        root.path(),
        "island",
        "name = \"island\"\n",
        &wired_source("island", None),
    )?;

    let mut registry = grouped(Sandbox::restricted().instruction_limit(10_000_000));
    assert!(registry.load_dir(root.path())?.is_clean());

    // The burn fits the shared allowance, and the allowance resets per call:
    // repeated burns never accumulate into a failure.
    for _ in 0..3 {
        assert_eq!(run_named(&registry, "user", "")?, "burned");
    }
    assert_eq!(run_named(&registry, "base", "")?, "base");
    assert_eq!(run_named(&registry, "island", "")?, "island");
    Ok(())
}

#[test]
fn grouped_isolation_is_reported_as_such() -> TestResult {
    let backend = LuaBackend::grouped(Sandbox::restricted());
    assert_eq!(backend.isolation_name(), "per-group");
    Ok(())
}

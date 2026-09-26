//! Per-plugin Lua states: standard library selection and resource limits.
#![cfg(feature = "registry")]

use std::fs;
use std::path::Path;

use stanchion_lua::lua_class;
use stanchion_lua::{Lua, Result, StdLib, Table};
use stanchion::tests::common::{first_failure, probe_source, write_plugin};

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;
type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[lua_class]
pub trait Probe {
    fn new(config: Table, deps: Table) -> Result<Self>;
    fn run(&self, input: String) -> Result<String>;
}

fn probe_source(body: &str) -> String {
    format!(
        r#"
local P = {{}}
P.__index = P
function P.new(config, deps) return setmetatable({{}}, P) end
function P:run(input)
  {body}
end
return P
"#
    )
}

fn write_plugin(root: &Path, name: &str, manifest: &str, source: &str) -> TestResult {
    let dir = root.join(name);
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("plugin.toml"), manifest)?;
    fs::write(dir.join("init.lua"), source)?;
    Ok(())
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
    let mut registry: Registry<ProbeClass> = Registry::isolated(Lua::new(), Sandbox::restricted());
    let report = registry.load_dir(root.path())?;
    assert!(report.is_clean(), "failures: {:?}", report.failures);

    let probe = registry.get("probe").ok_or("probe should load")?;
    // `os` and `io` are absent; `string` is present.
    assert_eq!(probe.instance().run(String::new())?, "nil/nil/table");
    Ok(())
}

#[test]
fn permissive_sandbox_grants_io_and_os() -> TestResult {
    let root = single_plugin(r#"return type(os) .. "/" .. type(io)"#)?;
    let mut registry: Registry<ProbeClass> = Registry::isolated(Lua::new(), Sandbox::permissive());
    registry.load_dir(root.path())?;

    let probe = registry.get("probe").ok_or("probe should load")?;
    assert_eq!(probe.instance().run(String::new())?, "table/table");
    Ok(())
}

#[test]
fn deny_list_removes_reachable_escapes() -> TestResult {
    let root = single_plugin(r#"return type(dofile) .. "/" .. type(package.loadlib)"#)?;
    let mut registry: Registry<ProbeClass> = Registry::isolated(Lua::new(), Sandbox::restricted());
    registry.load_dir(root.path())?;

    // `package` is loaded so `require` works, but `loadlib` would load any .so.
    let probe = registry.get("probe").ok_or("probe should load")?;
    assert_eq!(probe.instance().run(String::new())?, "nil/nil");
    Ok(())
}

#[test]
fn explicit_lib_selection_is_honoured() -> TestResult {
    let root = single_plugin(r#"return type(string) .. "/" .. type(math)"#)?;
    let mut registry: Registry<ProbeClass> = Registry::isolated(
        Lua::new(),
        Sandbox::restricted().libs(StdLib::STRING | StdLib::TABLE),
    );
    registry.load_dir(root.path())?;

    let probe = registry.get("probe").ok_or("probe should load")?;
    assert_eq!(probe.instance().run(String::new())?, "table/nil");
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

    let mut registry: Registry<ProbeClass> = Registry::isolated(Lua::new(), Sandbox::restricted());
    registry.load_dir(root.path())?;

    let writer = registry.get("writer").ok_or("writer should load")?;
    let reader = registry.get("reader").ok_or("reader should load")?;
    writer.instance().run(String::new())?;

    // A raw global write escapes the per-plugin environment, but not the state.
    assert_eq!(reader.instance().run(String::new())?, "nil");
    Ok(())
}

#[test]
fn memory_limit_is_enforced_per_plugin() -> TestResult {
    let root = single_plugin(
        r#"local t = {} for i = 1, 1e7 do t[i] = string.rep("x", 256) end return 'never'"#,
    )?;
    let mut registry: Registry<ProbeClass> = Registry::isolated(
        Lua::new(),
        Sandbox::restricted().memory_limit(8 * 1024 * 1024),
    );
    registry.load_dir(root.path())?;

    let probe = registry.get("probe").ok_or("probe should load")?;
    let Err(error) = probe.instance().run(String::new()) else {
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
    let mut registry: Registry<ProbeClass> =
        Registry::isolated(Lua::new(), Sandbox::restricted().instruction_limit(100_000));
    registry.load_dir(root.path())?;

    let probe = registry.get("probe").ok_or("probe should load")?;
    let Err(error) = probe.instance().run(String::new()) else {
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
    let mut registry: Registry<ProbeClass> =
        Registry::isolated(Lua::new(), Sandbox::restricted().instruction_limit(500_000));
    registry.load_dir(root.path())?;

    for round in 0..5 {
        let outcomes = registry.dispatch(|plugin| plugin.run(String::new()));
        let outcome = outcomes.first().ok_or("expected one outcome")?;
        assert_eq!(
            outcome.result.as_ref().map_err(|err| err.to_string())?,
            "done",
            "round {round} should not inherit the previous round's budget"
        );
    }
    Ok(())
}

#[test]
fn ambient_setup_installs_host_functions_into_every_state() -> TestResult {
    let root = single_plugin(r#"return host_greeting()"#)?;
    let mut registry: Registry<ProbeClass> = Registry::isolated(Lua::new(), Sandbox::restricted())
        .with_setup(|host| {
            host.ambient("host_greeting", |lua| {
                let greeting = lua.create_function(|_, ()| Ok("from the host"))?;
                lua.globals().set("host_greeting", greeting)
            });
            Ok(())
        });
    let report = registry.load_dir(root.path())?;
    assert!(report.is_clean(), "failures: {:?}", report.failures);

    let probe = registry.get("probe").ok_or("probe should load")?;
    assert_eq!(probe.instance().run(String::new())?, "from the host");
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

    let mut registry: Registry<ProbeClass> = Registry::isolated(Lua::new(), Sandbox::restricted());
    let report = registry.load_dir(root.path())?;

    assert_eq!(report.loaded, ["base"]);
    assert!(
        matches!(first_failure(&report)?, FailureReason::CrossStateDependency(name) if name == "base"),
        "got: {:?}",
        report.failures
    );
    Ok(())
}

#[test]
fn shared_isolation_remains_the_default() -> TestResult {
    let registry: Registry<ProbeClass> = Registry::new(Lua::new());
    assert!(matches!(registry.isolation(), Isolation::Shared));
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

    let mut registry: Registry<ProbeClass> = Registry::grouped(Lua::new(), Sandbox::restricted());
    let report = registry.load_dir(root.path())?;
    assert!(report.is_clean(), "got: {:?}", report.failures);

    // The export crossed, which is the whole point: under per-plugin isolation this
    // plugin cannot even load.
    let user = registry.get("user").ok_or("`user` should load")?;
    assert_eq!(user.instance().run(String::new())?, "from base");

    let base = registry.get("base").ok_or("`base` should load")?;
    assert_eq!(base.group(), user.group());
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

    let mut registry: Registry<ProbeClass> = Registry::grouped(Lua::new(), Sandbox::restricted());
    assert!(registry.load_dir(root.path())?.is_clean());

    let left = registry.get("left").ok_or("`left` should load")?;
    let right = registry.get("right").ok_or("`right` should load")?;
    assert_ne!(left.group(), right.group());

    // Neither sees the other's global, so the states really are separate.
    assert_eq!(left.instance().run(String::new())?, "left");
    assert_eq!(right.instance().run(String::new())?, "right");
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

    let mut registry: Registry<ProbeClass> = Registry::grouped(Lua::new(), Sandbox::restricted());
    let report = registry.load_dir(root.path())?;
    assert!(report.is_clean(), "got: {:?}", report.failures);

    let group_of = |name: &str| {
        registry
            .get(name)
            .map(|plugin| plugin.group())
            .ok_or_else(|| format!("`{name}` should load"))
    };
    let component = group_of("shared")?;
    for name in ["left", "right", "top"] {
        assert_eq!(group_of(name)?, component, "`{name}` should join the group");
    }
    assert_ne!(group_of("island")?, component);

    assert_eq!(
        registry
            .get("top")
            .ok_or("`top` should load")?
            .instance()
            .run(String::new())?,
        "right"
    );
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

    let mut registry: Registry<ProbeClass> =
        Registry::grouped(Lua::new(), Sandbox::restricted().instruction_limit(10_000_000));
    assert!(registry.load_dir(root.path())?.is_clean());

    let used = |name: &str| {
        registry
            .get(name)
            .and_then(|plugin| plugin.budget())
            .map(stanchion::registry::Budget::used)
            .ok_or_else(|| format!("`{name}` should have a budget"))
    };
    for name in ["base", "user", "island"] {
        assert_eq!(used(name)?, 0, "`{name}` should start unspent");
    }

    assert_eq!(
        registry
            .get("user")
            .ok_or("`user` should load")?
            .instance()
            .run(String::new())?,
        "burned"
    );

    // One counter for the group: `base` sees what `user` spent, the island does not.
    let spent = used("user")?;
    assert!(spent > 0, "the loop should have been charged");
    assert_eq!(used("base")?, spent);
    assert_eq!(used("island")?, 0);
    Ok(())
}

#[test]
fn grouped_isolation_is_reported_as_such() -> TestResult {
    let registry: Registry<ProbeClass> = Registry::grouped(Lua::new(), Sandbox::restricted());
    assert!(matches!(registry.isolation(), Isolation::PerGroup(_)));
    Ok(())
}

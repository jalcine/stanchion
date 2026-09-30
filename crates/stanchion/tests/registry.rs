//! Plugin registry: discovery, dependency chains, failure isolation, reload.
#![cfg(feature = "registry")]

mod common;

use std::fs;
use std::path::Path;

use common::{TestResult, probe_source, run_named, write_plugin};
use stanchion::registry::{FailureReason, LoadFailure, Outcome, Registry, Value};
use stanchion_lua::backend::LuaBackend;
use tempfile::TempDir;

/// Publishes an `exports` table that closes over its config, so a reload changes it.
const ALPHA: &str = r#"
local Alpha = {}
Alpha.__index = Alpha

function Alpha.new(config, deps)
  return setmetatable({ label = "alpha", greeting = config.greeting }, Alpha)
end

function Alpha:greet(who)
  return string.format("%s, %s", self.greeting, who)
end

function Alpha:ping() return "alpha" end

function Alpha:exports()
  local greeting = self.greeting
  return {
    decorate = function(text) return "[" .. greeting .. "] " .. text end,
  }
end

return Alpha
"#;

/// Captures alpha's exports at construction and never re-reads them.
const BETA: &str = r#"
local Beta = {}
Beta.__index = Beta

function Beta.new(config, deps)
  return setmetatable({ label = "beta", alpha = deps.alpha }, Beta)
end

function Beta:greet(who)
  return self.alpha.decorate(who)
end

function Beta:ping() return "beta" end

return Beta
"#;

fn simple_plugin(label: &str, greet_body: &str) -> String {
    format!(
        r#"
local P = {{}}
P.__index = P

function P.new(config, deps)
  return setmetatable({{ label = "{label}", deps = deps }}, P)
end

function P:greet(who)
  {greet_body}
end

function P:ping() return "{label}" end

return P
"#
    )
}

fn write_plugin_boxed(
    root: &Path,
    name: &str,
    manifest: &str,
    source: Option<&str>,
) -> TestResult {
    let dir = root.join(name);
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("plugin.toml"), manifest)?;
    if let Some(source) = source {
        fs::write(dir.join("init.lua"), source)?;
    }
    Ok(())
}

/// A plugin root holding a working dependency chain and one of every failure mode.
fn plugin_root() -> std::result::Result<TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let path = root.path();

    write_plugin_boxed(
        path,
        "alpha",
        "name = \"alpha\"\nversion = \"1.0.0\"\n\n[config]\ngreeting = \"hello\"\n",
        Some(ALPHA),
    )?;
    write_plugin_boxed(
        path,
        "beta",
        "name = \"beta\"\nversion = \"1.0.0\"\n\n[dependencies]\nalpha = \"^1.0\"\n",
        Some(BETA),
    )?;
    write_plugin_boxed(
        path,
        "grumpy",
        "name = \"grumpy\"\n",
        Some(&simple_plugin(
            "grumpy",
            r#"error("grumpy refuses to greet " .. who)"#,
        )),
    )?;
    write_plugin_boxed(
        path,
        "hopeful",
        "name = \"hopeful\"\n\n[dependencies]\nghost = { version = \"*\", optional = true }\n",
        Some(&simple_plugin(
            "hopeful",
            "return tostring(self.deps.ghost)",
        )),
    )?;
    write_plugin_boxed(
        path,
        "silent",
        "name = \"silent\"\n",
        Some(&simple_plugin("silent", r#"return "silent: " .. who"#)),
    )?;
    write_plugin_boxed(
        path,
        "needy",
        "name = \"needy\"\n\n[dependencies]\nsilent = \"*\"\n",
        Some(&simple_plugin("needy", r#"return who"#)),
    )?;
    write_plugin_boxed(
        path,
        "picky",
        "name = \"picky\"\n\n[dependencies]\nalpha = \"^2.0\"\n",
        Some(&simple_plugin("picky", r#"return who"#)),
    )?;
    write_plugin_boxed(
        path,
        "broken",
        "name = \"broken\"\n",
        Some("error(\"boom\")\n"),
    )?;
    write_plugin_boxed(
        path,
        "orphan",
        "name = \"orphan\"\n\n[dependencies]\nghost = \"*\"\n",
        Some(&simple_plugin("orphan", r#"return who"#)),
    )?;
    write_plugin_boxed(
        path,
        "cyclic_a",
        "name = \"cyclic_a\"\n\n[dependencies]\ncyclic_b = \"*\"\n",
        Some(&simple_plugin("cyclic_a", r#"return who"#)),
    )?;
    write_plugin_boxed(
        path,
        "cyclic_b",
        "name = \"cyclic_b\"\n\n[dependencies]\ncyclic_a = \"*\"\n",
        Some(&simple_plugin("cyclic_b", r#"return who"#)),
    )?;
    write_plugin_boxed(path, "malformed", "this is not toml\n", Some(ALPHA))?;
    Ok(root)
}

fn shared_registry() -> Registry {
    Registry::new().with_runtime(Box::new(LuaBackend::shared()))
}

fn loaded_registry(
    root: &TempDir,
) -> std::result::Result<Registry, Box<dyn std::error::Error>> {
    let mut registry = shared_registry();
    registry.load_dir(root.path())?;
    Ok(registry)
}

/// Calls `greet` on the named plugin, expecting a string.
fn greet(
    registry: &Registry,
    name: &str,
    who: &str,
) -> std::result::Result<String, Box<dyn std::error::Error>> {
    match registry.call(name, "greet", &[Value::Str(who.to_string())])? {
        Value::Str(text) => Ok(text),
        other => Err(format!("expected a string, got {other:?}").into()),
    }
}

/// Looks up why one plugin failed, reporting an unexpected success as an error.
fn reason_for<'a>(
    failures: &'a [LoadFailure],
    name: &str,
) -> std::result::Result<&'a FailureReason, Box<dyn std::error::Error>> {
    failures
        .iter()
        .find(|failure| failure.name == name)
        .map(|failure| &failure.reason)
        .ok_or_else(|| format!("expected `{name}` to fail, but it did not").into())
}

/// Looks up one dispatch outcome by plugin name.
fn outcome_for<'a>(
    outcomes: &'a [Outcome],
    name: &str,
) -> std::result::Result<&'a Outcome, Box<dyn std::error::Error>> {
    outcomes
        .iter()
        .find(|outcome| outcome.plugin == name)
        .ok_or_else(|| format!("no dispatch outcome for `{name}`").into())
}

#[test]
fn loads_dependencies_first_and_isolates_every_failure() -> TestResult {
    let root = plugin_root()?;
    let mut registry = shared_registry();
    let report = registry.load_dir(root.path())?;

    assert_eq!(
        report.loaded,
        ["alpha", "grumpy", "hopeful", "silent", "beta"],
        "dependents must follow what they are wired to"
    );
    assert!(!report.is_clean());

    let failures = &report.failures;
    assert!(matches!(
        reason_for(failures, "broken")?,
        FailureReason::Runtime(_)
    ));
    assert!(matches!(
        reason_for(failures, "malformed")?,
        FailureReason::Manifest(_)
    ));
    assert!(matches!(
        reason_for(failures, "orphan")?,
        FailureReason::MissingDependency(dep) if dep == "ghost"
    ));
    assert!(matches!(
        reason_for(failures, "needy")?,
        FailureReason::Runtime(reason) if reason.contains("did not publish")
    ));
    assert!(matches!(
        reason_for(failures, "picky")?,
        FailureReason::IncompatibleDependency { name, required, found }
            if name == "alpha" && required == "^2.0" && found == "1.0.0"
    ));
    for name in ["cyclic_a", "cyclic_b"] {
        assert!(matches!(
            reason_for(failures, name)?,
            FailureReason::DependencyCycle(_)
        ));
    }
    Ok(())
}

#[test]
fn exports_reach_the_dependent() -> TestResult {
    let root = plugin_root()?;
    let registry = loaded_registry(&root)?;

    // beta greets by calling into alpha's published `decorate`.
    assert_eq!(greet(&registry, "beta", "world")?, "[hello] world");
    Ok(())
}

#[test]
fn only_exports_are_visible_to_a_dependent() -> TestResult {
    let root = tempfile::tempdir()?;
    write_plugin(
        root.path(),
        "alpha",
        "name = \"alpha\"\nversion = \"1.0.0\"\n",
        ALPHA,
    )?;
    // A dependent can see `decorate` but not instance internals: `greet` and
    // `greeting` live on the instance, not in `exports`.
    write_plugin(
        root.path(),
        "snoop",
        "name = \"snoop\"\n\n[dependencies]\nalpha = \"^1.0\"\n",
        &probe_source(
            r#"return type(self.deps.alpha.decorate) .. "/" .. tostring(self.deps.alpha.greet) .. "/" .. tostring(self.deps.alpha.greeting)"#,
        ),
    )?;

    let mut registry = shared_registry();
    let report = registry.load_dir(root.path())?;
    assert!(report.is_clean(), "failures: {:?}", report.failures);

    assert_eq!(
        run_named(&registry, "snoop", "x")?,
        "function/nil/nil"
    );
    Ok(())
}

#[test]
fn a_dependent_cannot_tamper_with_the_exports_proxy() -> TestResult {
    // #32: a dependent holds a read-only proxy. It must not be able to write through it
    // into the provider's live exports, nor reach the forwarding metatable to repoint
    // reads — either would hijack the provider's surface for every other dependent.
    let root = tempfile::tempdir()?;
    write_plugin(
        root.path(),
        "alpha",
        "name = \"alpha\"\nversion = \"1.0.0\"\n\n[config]\ngreeting = \"hello\"\n",
        ALPHA,
    )?;
    write_plugin(
        root.path(),
        "sneaky",
        "name = \"sneaky\"\n\n[dependencies]\nalpha = \"^1.0\"\n",
        &probe_source(
            r#"local seen = {}
            seen.metatable = getmetatable(self.deps.alpha)
            if type(seen.metatable) == "string" then return "hidden" else return "exposed" end"#,
        ),
    )?;
    write_plugin(
        root.path(),
        "witness",
        "name = \"witness\"\n\n[dependencies]\nalpha = \"^1.0\"\n",
        &probe_source(r#"return self.deps.alpha.decorate(input)"#),
    )?;

    let mut registry = shared_registry();
    let report = registry.load_dir(root.path())?;
    assert!(report.is_clean(), "failures: {:?}", report.failures);

    // The forwarding metatable is hidden behind __metatable.
    assert_eq!(run_named(&registry, "sneaky", "x")?, "hidden");

    // Writing through the proxy is refused rather than reaching the table.
    let attacking = probe_source(
        r#"local ok, err = pcall(function() self.deps.alpha.decorate = function() return "pwned" end end)
            if ok then return "wrote" else return tostring(err) end"#,
    );
    fs::write(root.path().join("sneaky").join("init.lua"), attacking)?;
    registry.reload("sneaky")?;
    assert!(
        run_named(&registry, "sneaky", "x")?.contains("read-only"),
        "writing through the proxy must fail"
    );

    // The genuine export still resolves and is unchanged for other dependents.
    assert_eq!(run_named(&registry, "witness", "hi")?, "[hello] hi");
    Ok(())
}

#[test]
fn reload_propagates_through_the_dependency_chain() -> TestResult {
    let root = plugin_root()?;
    let mut registry = shared_registry();
    registry.load_dir(root.path())?;

    assert_eq!(greet(&registry, "beta", "world")?, "[hello] world");

    fs::write(
        root.path().join("alpha").join("plugin.toml"),
        "name = \"alpha\"\nversion = \"1.1.0\"\n\n[config]\ngreeting = \"howdy\"\n",
    )?;
    registry.reload("alpha")?;

    // The proxy beta captured at construction now forwards to alpha's new exports.
    assert_eq!(greet(&registry, "beta", "world")?, "[howdy] world");
    assert_eq!(greet(&registry, "alpha", "world")?, "howdy, world");
    Ok(())
}

#[test]
fn absent_optional_dependency_leaves_a_nil_slot() -> TestResult {
    let root = plugin_root()?;
    let registry = loaded_registry(&root)?;

    assert_eq!(greet(&registry, "hopeful", "x")?, "nil");
    Ok(())
}

#[test]
fn passes_manifest_config_to_the_constructor() -> TestResult {
    let root = plugin_root()?;
    let registry = loaded_registry(&root)?;

    assert_eq!(greet(&registry, "alpha", "world")?, "hello, world");
    let version = registry
        .get("alpha")
        .ok_or("alpha should load")?
        .manifest()
        .version
        .as_ref()
        .ok_or("alpha should declare a version")?;
    assert_eq!(version.to_string(), "1.0.0");
    Ok(())
}

#[test]
fn plugin_writes_stay_out_of_the_shared_globals() -> TestResult {
    let root = tempfile::tempdir()?;
    write_plugin(
        root.path(),
        "alpha",
        "name = \"alpha\"\n\n[config]\ngreeting = \"hello\"\n",
        ALPHA,
    )?;
    // Reads the (possibly leaked) global rather than any capability.
    write_plugin(
        root.path(),
        "reader",
        "name = \"reader\"\n",
        &probe_source(r#"return tostring(leaked_global)"#),
    )?;

    let mut registry = shared_registry();
    let report = registry.load_dir(root.path())?;
    assert!(report.is_clean(), "failures: {:?}", report.failures);

    // `alpha` assigns `leaked_global` at chunk scope; the environment keeps it local.
    assert_eq!(run_named(&registry, "reader", "x")?, "nil");

    // Reads still reach the real globals, or `string.format` in `greet` would fail.
    assert_eq!(greet(&registry, "alpha", "you")?, "hello, you");
    Ok(())
}

#[test]
fn dispatch_reports_each_plugin_separately() -> TestResult {
    let root = plugin_root()?;
    let registry = loaded_registry(&root)?;

    let outcomes = registry.dispatch("greet", &[Value::Str("world".to_string())]);

    let alpha = outcome_for(&outcomes, "alpha")?;
    assert_eq!(alpha.value, Some(Value::Str("hello, world".to_string())));
    let beta = outcome_for(&outcomes, "beta")?;
    assert_eq!(beta.value, Some(Value::Str("[hello] world".to_string())));

    let grumpy = outcome_for(&outcomes, "grumpy")?;
    let error = grumpy.error.as_ref().ok_or("grumpy was expected to fail")?;
    assert!(
        error.contains("grumpy refuses to greet world"),
        "got: {error}"
    );
    Ok(())
}

#[test]
fn reloading_an_unknown_plugin_is_an_error() -> TestResult {
    let root = plugin_root()?;
    let mut registry = loaded_registry(&root)?;
    let Err(error) = registry.reload("nope") else {
        return Err("reloading an unknown plugin should fail".into());
    };
    let error = error.to_string();
    assert!(error.contains("no plugin named `nope`"), "got: {error}");
    Ok(())
}

#[cfg(feature = "async")]
#[tokio::test]
async fn dispatch_async_awaits_every_plugin() -> TestResult {
    let root = plugin_root()?;
    let registry = loaded_registry(&root)?;

    let outcomes = registry.dispatch_async("ping", &[]).await;
    let names: Vec<&str> = outcomes.iter().map(|outcome| outcome.plugin.as_str()).collect();
    assert_eq!(names, ["alpha", "grumpy", "hopeful", "silent", "beta"]);
    assert!(outcomes.iter().all(|outcome| outcome.error.is_none()));
    Ok(())
}

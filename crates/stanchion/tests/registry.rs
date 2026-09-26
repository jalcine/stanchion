//! Plugin registry: discovery, dependency chains, failure isolation, config, reload.
#![cfg(feature = "registry")]

use std::fs;
use std::path::Path;

use stanchion_lua::lua_class;
use stanchion_lua::mlua::{Lua, Result, Table};
use stanchion::registry::{FailureReason, LoadFailure, Outcome, Registry};
use tempfile::TempDir;

/// Tests report failures as errors rather than panicking, so a broken assumption
/// surfaces with its own message instead of a bare unwrap location.
type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

#[lua_class]
pub trait Greeter {
    /// The registry calls this by convention with `(config, deps)`.
    fn new(config: Table, deps: Table) -> Result<Self>;

    fn greet(&self, who: String) -> Result<String>;

    #[lua(field)]
    fn label(&self) -> Result<String>;

    #[cfg(feature = "async")]
    async fn ping(&self) -> Result<String>;
}

/// Publishes an `exports` table that closes over its config, so a reload changes it.
const ALPHA: &str = r#"
leaked_global = "should not escape"

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

fn write_plugin(
    root: &Path,
    name: &str,
    manifest: &str,
    source: Option<&str>,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
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

    write_plugin(
        path,
        "alpha",
        "name = \"alpha\"\nversion = \"1.0.0\"\n\n[config]\ngreeting = \"hello\"\n",
        Some(ALPHA),
    )?;
    write_plugin(
        path,
        "beta",
        "name = \"beta\"\nversion = \"1.0.0\"\n\n[dependencies]\nalpha = \"^1.0\"\n",
        Some(BETA),
    )?;
    write_plugin(
        path,
        "grumpy",
        "name = \"grumpy\"\n",
        Some(&simple_plugin(
            "grumpy",
            r#"error("grumpy refuses to greet " .. who)"#,
        )),
    )?;
    write_plugin(
        path,
        "hopeful",
        "name = \"hopeful\"\n\n[dependencies]\nghost = { version = \"*\", optional = true }\n",
        Some(&simple_plugin(
            "hopeful",
            "return tostring(self.deps.ghost)",
        )),
    )?;
    write_plugin(
        path,
        "silent",
        "name = \"silent\"\n",
        Some(&simple_plugin("silent", r#"return "silent: " .. who"#)),
    )?;
    write_plugin(
        path,
        "needy",
        "name = \"needy\"\n\n[dependencies]\nsilent = \"*\"\n",
        Some(&simple_plugin("needy", r#"return who"#)),
    )?;
    write_plugin(
        path,
        "picky",
        "name = \"picky\"\n\n[dependencies]\nalpha = \"^2.0\"\n",
        Some(&simple_plugin("picky", r#"return who"#)),
    )?;
    write_plugin(
        path,
        "broken",
        "name = \"broken\"\n",
        Some("error(\"boom\")\n"),
    )?;
    write_plugin(
        path,
        "orphan",
        "name = \"orphan\"\n\n[dependencies]\nghost = \"*\"\n",
        Some(&simple_plugin("orphan", r#"return who"#)),
    )?;
    write_plugin(
        path,
        "cyclic_a",
        "name = \"cyclic_a\"\n\n[dependencies]\ncyclic_b = \"*\"\n",
        Some(&simple_plugin("cyclic_a", r#"return who"#)),
    )?;
    write_plugin(
        path,
        "cyclic_b",
        "name = \"cyclic_b\"\n\n[dependencies]\ncyclic_a = \"*\"\n",
        Some(&simple_plugin("cyclic_b", r#"return who"#)),
    )?;
    write_plugin(path, "malformed", "this is not toml\n", Some(ALPHA))?;
    Ok(root)
}

fn loaded_registry(
    root: &TempDir,
) -> std::result::Result<Registry<GreeterClass>, Box<dyn std::error::Error>> {
    let mut registry = Registry::new(Lua::new());
    registry.load_dir(root.path())?;
    Ok(registry)
}

/// Looks up one plugin, reporting a missing one as an error.
fn plugin<'a>(
    registry: &'a Registry<GreeterClass>,
    name: &str,
) -> std::result::Result<&'a stanchion::registry::Plugin<GreeterClass>, Box<dyn std::error::Error>>
{
    registry
        .get(name)
        .ok_or_else(|| format!("expected `{name}` to be loaded").into())
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
fn outcome_for<'a, R>(
    outcomes: &'a [Outcome<'a, R>],
    name: &str,
) -> std::result::Result<&'a Result<R>, Box<dyn std::error::Error>> {
    outcomes
        .iter()
        .find(|outcome| outcome.name == name)
        .map(|outcome| &outcome.result)
        .ok_or_else(|| format!("no dispatch outcome for `{name}`").into())
}

#[test]
fn loads_dependencies_first_and_isolates_every_failure() -> TestResult {
    let root = plugin_root()?;
    let mut registry: Registry<GreeterClass> = Registry::new(Lua::new());
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
        FailureReason::Lua(_)
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
        FailureReason::MissingExports(dep) if dep == "silent"
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
    let beta = plugin(&registry, "beta")?;
    assert_eq!(beta.instance().greet("world".to_string())?, "[hello] world");
    Ok(())
}

#[test]
fn only_exports_are_visible_to_a_dependent() -> TestResult {
    let root = plugin_root()?;
    let registry = loaded_registry(&root)?;

    let proxy = plugin(&registry, "alpha")?
        .exports()
        .ok_or("alpha should publish exports")?;
    assert!(proxy.get::<Option<mlua::Function>>("decorate")?.is_some());
    // `greet` and `greeting` are instance internals, not published.
    assert_eq!(proxy.get::<Option<mlua::Value>>("greet")?, None);
    assert_eq!(proxy.get::<Option<String>>("greeting")?, None);
    Ok(())
}

#[test]
fn reload_propagates_through_the_dependency_chain() -> TestResult {
    let root = plugin_root()?;
    let mut registry: Registry<GreeterClass> = Registry::new(Lua::new());
    registry.load_dir(root.path())?;

    // Hold the *original* beta instance: it is never rebuilt below.
    let beta = plugin(&registry, "beta")?.instance().clone();
    assert_eq!(beta.greet("world".to_string())?, "[hello] world");

    fs::write(
        root.path().join("alpha").join("plugin.toml"),
        "name = \"alpha\"\nversion = \"1.1.0\"\n\n[config]\ngreeting = \"howdy\"\n",
    )?;
    registry.reload("alpha")?;

    // The proxy beta captured at construction now forwards to alpha's new exports.
    assert_eq!(beta.greet("world".to_string())?, "[howdy] world");
    assert_eq!(
        plugin(&registry, "alpha")?
            .instance()
            .greet("world".to_string())?,
        "howdy, world"
    );
    Ok(())
}

#[test]
fn absent_optional_dependency_leaves_a_nil_slot() -> TestResult {
    let root = plugin_root()?;
    let registry = loaded_registry(&root)?;

    let hopeful = plugin(&registry, "hopeful")?;
    assert_eq!(hopeful.instance().greet("x".to_string())?, "nil");
    Ok(())
}

#[test]
fn passes_manifest_config_to_the_constructor() -> TestResult {
    let root = plugin_root()?;
    let registry = loaded_registry(&root)?;

    let alpha = plugin(&registry, "alpha")?;
    assert_eq!(alpha.instance().greet("world".to_string())?, "hello, world");
    assert_eq!(alpha.instance().label()?, "alpha");
    let version = alpha
        .manifest()
        .version
        .as_ref()
        .ok_or("alpha should declare a version")?;
    assert_eq!(version.to_string(), "1.0.0");
    Ok(())
}

#[test]
fn plugin_writes_stay_out_of_the_shared_globals() -> TestResult {
    let root = plugin_root()?;
    let registry = loaded_registry(&root)?;

    // `alpha` assigns `leaked_global` at chunk scope; the environment keeps it local.
    let leaked: Option<String> = registry.lua().globals().get("leaked_global")?;
    assert_eq!(leaked, None);

    // Reads still reach the real globals, or `string.format` in `greet` would fail.
    assert_eq!(
        plugin(&registry, "alpha")?
            .instance()
            .greet("you".to_string())?,
        "hello, you"
    );
    Ok(())
}

#[test]
fn dispatch_reports_each_plugin_separately() -> TestResult {
    let root = plugin_root()?;
    let registry = loaded_registry(&root)?;

    let outcomes = registry.dispatch(|plugin| plugin.greet("world".to_string()));

    let alpha = outcome_for(&outcomes, "alpha")?;
    assert_eq!(
        alpha.as_ref().map_err(|err| err.to_string())?,
        "hello, world"
    );
    let beta = outcome_for(&outcomes, "beta")?;
    assert_eq!(
        beta.as_ref().map_err(|err| err.to_string())?,
        "[hello] world"
    );

    let Err(error) = outcome_for(&outcomes, "grumpy")? else {
        return Err("grumpy was expected to fail".into());
    };
    let error = error.to_string();
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

    let outcomes = registry.dispatch_async(|plugin| plugin.ping()).await;
    let names: Vec<&str> = outcomes.iter().map(|outcome| outcome.name).collect();
    assert_eq!(names, ["alpha", "grumpy", "hopeful", "silent", "beta"]);
    assert!(outcomes.iter().all(|outcome| outcome.result.is_ok()));
    Ok(())
}

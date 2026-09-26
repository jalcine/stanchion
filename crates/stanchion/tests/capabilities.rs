//! Declared capabilities: request in the manifest, grant by policy, bind in the
//! plugin's environment.
#![cfg(feature = "registry")]

mod common;

use std::fs;

use stanchion::registry::{
    CapabilityRequest, Decision, FailureReason, Registry, Rules, Sandbox, toml,
};
use stanchion_lua::lua_class;
use stanchion_lua::mlua::{self, Lua, Result, Table};
use tempfile::TempDir;

use common::{
    Fallible, TestResult, first_failure, lua_function, probe_source, with_lua, write_plugin,
};

#[lua_class]
pub trait Probe {
    fn new(config: Table, deps: Table) -> Result<Self>;
    fn run(&self, input: String) -> Result<String>;
}

fn single(manifest: &str, body: &str) -> Fallible<TempDir> {
    let root = tempfile::tempdir()?;
    write_plugin(root.path(), "probe", manifest, &probe_source(body))?;
    Ok(root)
}

/// A host offering `log` (ungated params) and `network` (an allowlist the provider
/// enforces from its grant).
fn registry() -> Registry<ProbeClass> {
    Registry::isolated(Lua::new(), Sandbox::restricted()).with_setup(|host| {
        host.capability("log", |runtime, _grant| {
            lua_function(runtime, |_, message: String| Ok(format!("logged: {message}")))
        });
        host.capability("network", |runtime, grant| {
            // The allowlist is baked in here, so the plugin cannot widen it later.
            let allowed: Vec<String> = grant.get_or_default("hosts");
            lua_function(runtime, move |_, host: String| {
                if allowed.contains(&host) {
                    Ok(format!("fetched {host}"))
                } else {
                    Err(mlua::Error::RuntimeError(format!("`{host}` not granted")))
                }
            })
        });
        Ok(())
    })
}

#[test]
fn a_granted_capability_is_bound_in_the_environment() -> TestResult {
    let root = single(
        "name = \"probe\"\n\n[capabilities.log]\n",
        r#"return log(input)"#,
    )?;
    let mut registry = registry().with_policy(Rules::deny_all().allow("log"));
    let report = registry.load_dir(root.path())?;
    assert!(report.is_clean(), "failures: {:?}", report.failures);

    let probe = registry.get("probe").ok_or("probe should load")?;
    assert_eq!(probe.instance().run("hello".to_string())?, "logged: hello");
    Ok(())
}

#[test]
fn an_undeclared_capability_is_simply_absent() -> TestResult {
    // The host offers `log`, policy allows it, but this plugin never asked.
    let root = single("name = \"probe\"\n", r#"return type(log)"#)?;
    let mut registry = registry().with_policy(Rules::deny_all().allow("log"));
    registry.load_dir(root.path())?;

    let probe = registry.get("probe").ok_or("probe should load")?;
    assert_eq!(probe.instance().run(String::new())?, "nil");
    Ok(())
}

#[test]
fn deny_by_default_refuses_even_a_registered_provider() -> TestResult {
    let root = single(
        "name = \"probe\"\n\n[capabilities.log]\n",
        r#"return log(input)"#,
    )?;
    // Provider registered, but no policy at all.
    let mut registry = registry();
    let report = registry.load_dir(root.path())?;

    assert!(report.loaded.is_empty());
    assert!(
        matches!(first_failure(&report)?, FailureReason::CapabilityDenied { name, .. } if name == "log"),
        "got: {:?}",
        report.failures
    );
    Ok(())
}

#[test]
fn policy_can_narrow_the_requested_parameters() -> TestResult {
    // The plugin asks for two hosts; policy grants only one.
    let root = single(
        "name = \"probe\"\n\n[capabilities.network]\nhosts = [\"allowed.example\", \"evil.example\"]\n",
        r#"return fetch(input)"#,
    )?;
    let mut registry = registry().with_policy(Rules::deny_all().allow_with(
        "network",
        |_request: &CapabilityRequest| {
            let mut narrowed = toml::Table::new();
            narrowed.insert(
                "hosts".to_string(),
                toml::Value::Array(vec![toml::Value::String("allowed.example".to_string())]),
            );
            Decision::GrantWith(narrowed)
        },
    ));

    // The capability binds under the name the manifest used.
    let root_source = probe_source(r#"return network(input)"#);
    fs::write(root.path().join("probe").join("init.lua"), root_source)?;
    registry.load_dir(root.path())?;

    let probe = registry.get("probe").ok_or("probe should load")?;
    assert_eq!(
        probe.instance().run("allowed.example".to_string())?,
        "fetched allowed.example"
    );

    let Err(error) = probe.instance().run("evil.example".to_string()) else {
        return Err("the narrowed grant should refuse the host policy removed".into());
    };
    assert!(error.to_string().contains("not granted"), "got: {error}");
    Ok(())
}

#[test]
fn an_unknown_capability_fails_the_plugin() -> TestResult {
    let root = single(
        "name = \"probe\"\n\n[capabilities.telepathy]\n",
        r#"return "x""#,
    )?;
    let mut registry = registry().with_policy(Rules::deny_all().allow("telepathy"));
    let report = registry.load_dir(root.path())?;

    assert!(
        matches!(first_failure(&report)?, FailureReason::UnknownCapability(name) if name == "telepathy"),
        "got: {:?}",
        report.failures
    );
    Ok(())
}

#[test]
fn an_optional_capability_is_skipped_when_denied() -> TestResult {
    let root = single(
        "name = \"probe\"\n\n[capabilities.log]\noptional = true\n",
        r#"return type(log)"#,
    )?;
    // No policy: `log` is denied, but optional, so the plugin still loads.
    let mut registry = registry();
    let report = registry.load_dir(root.path())?;
    assert!(report.is_clean(), "failures: {:?}", report.failures);

    let probe = registry.get("probe").ok_or("probe should load")?;
    assert_eq!(probe.instance().run(String::new())?, "nil");
    assert_eq!(probe.granted_capabilities().count(), 0);
    Ok(())
}

#[test]
fn the_reserved_optional_key_never_reaches_the_provider() -> TestResult {
    let root = single(
        "name = \"probe\"\n\n[capabilities.network]\nhosts = [\"a.example\"]\noptional = true\n",
        r#"return network("a.example")"#,
    )?;
    let mut registry = registry().with_policy(Rules::deny_all().allow_with(
        "network",
        |request: &CapabilityRequest| {
            assert!(request.optional, "optional should be parsed out");
            assert!(
                !request.params.contains_key("optional"),
                "optional should be stripped"
            );
            Decision::Grant
        },
    ));
    registry.load_dir(root.path())?;

    let probe = registry.get("probe").ok_or("probe should load")?;
    assert_eq!(probe.instance().run(String::new())?, "fetched a.example");
    Ok(())
}

#[test]
fn granted_capabilities_are_introspectable() -> TestResult {
    let root = single(
        "name = \"probe\"\n\n[capabilities.log]\n\n[capabilities.network]\nhosts = []\n",
        r#"return "x""#,
    )?;
    let mut registry = registry().with_policy(Rules::deny_all().allow("log").allow("network"));
    registry.load_dir(root.path())?;

    let probe = registry.get("probe").ok_or("probe should load")?;
    let granted: Vec<&str> = probe.granted_capabilities().collect();
    assert_eq!(granted, ["log", "network"]);
    Ok(())
}

#[test]
fn revoking_unbinds_a_live_capability() -> TestResult {
    let root = single(
        "name = \"probe\"\n\n[capabilities.log]\n",
        r#"if log == nil then return "revoked" end return log(input)"#,
    )?;
    let mut registry = registry().with_policy(Rules::deny_all().allow("log"));
    registry.load_dir(root.path())?;

    {
        let probe = registry.get("probe").ok_or("probe should load")?;
        assert_eq!(probe.instance().run("hi".to_string())?, "logged: hi");
    }

    assert!(registry.revoke("probe", "log")?);
    assert!(
        !registry.revoke("probe", "log")?,
        "a second revoke is a no-op"
    );

    let probe = registry.get("probe").ok_or("probe should load")?;
    assert_eq!(probe.instance().run("hi".to_string())?, "revoked");
    assert_eq!(probe.granted_capabilities().count(), 0);
    Ok(())
}

#[test]
fn a_submodule_sees_its_plugins_capabilities() -> TestResult {
    // The gap per-plugin `require` closes: a plugin split across files must not
    // half-see what it was granted.
    let root = tempfile::tempdir()?;
    write_plugin(
        root.path(),
        "probe",
        "name = \"probe\"\n\n[capabilities.log]\n",
        &probe_source(r#"local helper = require("helper") return helper.shout(input)"#),
    )?;
    fs::write(
        root.path().join("probe").join("helper.lua"),
        "return { shout = function(text) return log(text:upper()) end }\n",
    )?;

    let mut registry = registry().with_policy(Rules::deny_all().allow("log"));
    let report = registry.load_dir(root.path())?;
    assert!(report.is_clean(), "failures: {:?}", report.failures);

    let probe = registry.get("probe").ok_or("probe should load")?;
    assert_eq!(probe.instance().run("hi".to_string())?, "logged: HI");
    Ok(())
}

#[test]
fn audit_reports_requests_without_running_anything() -> TestResult {
    let root = tempfile::tempdir()?;
    write_plugin(
        root.path(),
        "good",
        "name = \"good\"\n\n[capabilities.log]\n",
        &probe_source(r#"return "x""#),
    )?;
    write_plugin(
        root.path(),
        "greedy",
        "name = \"greedy\"\n\n[capabilities.telepathy]\n",
        // Would blow up if it ever ran.
        "error('audit must not execute plugin code')\n",
    )?;

    let registry = registry().with_policy(Rules::deny_all().allow("log"));
    let audit = registry.audit(root.path())?;

    assert_eq!(audit.plugins.len(), 2);
    let unsatisfiable: Vec<&str> = audit
        .unsatisfiable()
        .map(|request| request.name.as_str())
        .collect();
    assert_eq!(unsatisfiable, ["telepathy"]);
    assert!(audit.offered.contains(&"network".to_string()));
    Ok(())
}

#[test]
fn audit_lists_ambient_globals_alongside_declarations() -> TestResult {
    let root = single("name = \"probe\"\n", r#"return "x""#)?;
    let registry: Registry<ProbeClass> = Registry::isolated(Lua::new(), Sandbox::restricted())
        .with_setup(|host| {
            host.ambient("HOST_VERSION", |runtime| {
                with_lua(runtime, |lua| lua.globals().set("HOST_VERSION", "1.0"))
            });
            Ok(())
        });

    let audit = registry.audit(root.path())?;
    // Ambient values reach a plugin whether or not it declared anything.
    assert_eq!(audit.ambient, ["HOST_VERSION"]);
    Ok(())
}

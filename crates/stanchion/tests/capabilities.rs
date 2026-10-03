//! Declared capabilities: request in the manifest, grant by policy, bind in the
//! plugin's environment.
#![cfg(feature = "registry")]

mod common;

use std::collections::BTreeMap;
use std::fs;

use stanchion::registry::{CapabilityCall, CapabilityRequest, Decision, FailureReason, Rules, Value};
use tempfile::TempDir;

use common::{Fallible, TestResult, first_failure, probe_source, registry, run, write_plugin};

fn single(manifest: &str, body: &str) -> Fallible<TempDir> {
    let root = tempfile::tempdir()?;
    write_plugin(root.path(), "probe", manifest, &probe_source(body))?;
    Ok(root)
}

/// A host offering `log` (ungated params) and `network` (an allowlist the provider
/// enforces from its grant).
fn offering() -> stanchion::registry::Registry {
    registry().with_setup(|host| {
        host.capability("log", |call: &CapabilityCall| {
            let message = match call.args.first() {
                Some(Value::Str(text)) => text.clone(),
                _ => "(no message)".to_string(),
            };
            Ok(Value::Str(format!("logged: {message}")))
        });
        host.capability("network", |call: &CapabilityCall| {
            // The allowlist is baked in here, so the plugin cannot widen it later.
            let allowed: Vec<String> = call.grant.get_or_default("hosts");
            let host = match call.args.first() {
                Some(Value::Str(host)) => host.clone(),
                _ => return Err("expected a host string".to_string()),
            };
            if allowed.contains(&host) {
                Ok(Value::Str(format!("fetched {host}")))
            } else {
                Err(format!("`{host}` not granted"))
            }
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
    let mut registry = offering().with_policy(Rules::deny_all().allow("log"));
    let report = registry.load_dir(root.path())?;
    assert!(report.is_clean(), "failures: {:?}", report.failures);

    assert_eq!(run(&registry, "hello")?, "logged: hello");
    Ok(())
}

#[test]
fn an_undeclared_capability_is_simply_absent() -> TestResult {
    // The host offers `log`, policy allows it, but this plugin never asked.
    let root = single("name = \"probe\"\n", r#"return type(log)"#)?;
    let mut registry = offering().with_policy(Rules::deny_all().allow("log"));
    registry.load_dir(root.path())?;

    assert_eq!(run(&registry, "")?, "nil");
    Ok(())
}

#[test]
fn deny_by_default_refuses_even_a_registered_provider() -> TestResult {
    let root = single(
        "name = \"probe\"\n\n[capabilities.log]\n",
        r#"return log(input)"#,
    )?;
    // Provider registered, but no policy at all.
    let mut registry = offering();
    let report = registry.load_dir(root.path())?;

    assert!(report.loaded.is_empty());
    assert!(
        matches!(first_failure(&report)?, FailureReason::Runtime(reason) if reason.contains("`log` is not granted by policy")),
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
    let mut registry = offering().with_policy(Rules::deny_all().allow_with(
        "network",
        |_request: &CapabilityRequest| {
            let mut narrowed = BTreeMap::new();
            narrowed.insert(
                "hosts".to_string(),
                Value::List(vec![Value::Str("allowed.example".to_string())]),
            );
            Decision::GrantWith(Value::Map(narrowed))
        },
    ));

    // The capability binds under the name the manifest used.
    let root_source = probe_source(r#"return network(input)"#);
    fs::write(root.path().join("probe").join("init.lua"), root_source)?;
    registry.load_dir(root.path())?;

    assert_eq!(run(&registry, "allowed.example")?, "fetched allowed.example");

    let Err(error) = registry.call(
        "probe",
        "run",
        &[Value::Str("evil.example".to_string())],
    ) else {
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
    let mut registry = offering().with_policy(Rules::deny_all().allow("telepathy"));
    let report = registry.load_dir(root.path())?;

    assert!(
        matches!(first_failure(&report)?, FailureReason::Runtime(reason) if reason.contains("`telepathy`") && reason.contains("does not offer")),
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
    let mut registry = offering();
    let report = registry.load_dir(root.path())?;
    assert!(report.is_clean(), "failures: {:?}", report.failures);

    assert_eq!(run(&registry, "")?, "nil");
    assert_eq!(registry.get("probe").map(|p| p.granted_capabilities().count()), Some(0));
    Ok(())
}

#[test]
fn the_reserved_optional_key_never_reaches_the_provider() -> TestResult {
    let root = single(
        "name = \"probe\"\n\n[capabilities.network]\nhosts = [\"a.example\"]\noptional = true\n",
        r#"return network("a.example")"#,
    )?;
    let mut registry = offering().with_policy(Rules::deny_all().allow_with(
        "network",
        |request: &CapabilityRequest| {
            assert!(request.optional, "optional should be parsed out");
            assert!(
                matches!(&request.params, Value::Map(params) if !params.contains_key("optional")),
                "optional should be stripped"
            );
            Decision::Grant
        },
    ));
    registry.load_dir(root.path())?;

    assert_eq!(run(&registry, "")?, "fetched a.example");
    Ok(())
}

#[test]
fn granted_capabilities_are_introspectable() -> TestResult {
    let root = single(
        "name = \"probe\"\n\n[capabilities.log]\n\n[capabilities.network]\nhosts = []\n",
        r#"return "x""#,
    )?;
    let mut registry = offering().with_policy(Rules::deny_all().allow("log").allow("network"));
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
    let mut registry = offering().with_policy(Rules::deny_all().allow("log"));
    registry.load_dir(root.path())?;

    assert_eq!(run(&registry, "hi")?, "logged: hi");

    assert!(registry.revoke("probe", "log")?);
    assert!(
        !registry.revoke("probe", "log")?,
        "a second revoke is a no-op"
    );

    assert_eq!(run(&registry, "hi")?, "revoked");
    assert_eq!(registry.get("probe").map(|p| p.granted_capabilities().count()), Some(0));
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

    let mut registry = offering().with_policy(Rules::deny_all().allow("log"));
    let report = registry.load_dir(root.path())?;
    assert!(report.is_clean(), "failures: {:?}", report.failures);

    assert_eq!(run(&registry, "hi")?, "logged: HI");
    Ok(())
}

#[test]
fn require_ignores_plugin_controlled_search_paths() -> TestResult {
    // #31: setting `package.path` at runtime must not steer `require` at a file outside
    // the plugin's own directory. A file the host process can read but never blessed
    // stays unreachable.
    let outside = tempfile::tempdir()?;
    fs::write(
        outside.path().join("secret.lua"),
        "return { value = \"leaked\" }\n",
    )?;

    let root = tempfile::tempdir()?;
    let body = format!(
        "package.path = \"{dir}/?.lua;\" .. package.path\n\
         local ok, mod = pcall(require, \"secret\")\n\
         if ok and type(mod) == \"table\" then return \"LEAKED:\" .. tostring(mod.value) end\n\
         return \"blocked\"",
        dir = outside.path().display()
    );
    write_plugin(
        root.path(),
        "probe",
        "name = \"probe\"\n",
        &probe_source(&body),
    )?;

    let mut registry = offering().with_policy(Rules::deny_all());
    let report = registry.load_dir(root.path())?;
    assert!(report.is_clean(), "failures: {:?}", report.failures);

    assert_eq!(
        run(&registry, "hi")?,
        "blocked",
        "a plugin-set package.path must not let require reach outside its directory"
    );
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

    let registry = offering().with_policy(Rules::deny_all().allow("log"));
    let audit = registry.audit(root.path())?;

    assert_eq!(audit.plugins.len(), 2);
    let unsatisfiable: Vec<&str> = audit
        .unsatisfiable()
        .map(|request| request.capability.as_str())
        .collect();
    assert_eq!(unsatisfiable, ["telepathy"]);
    assert!(audit.offered.contains(&"network".to_string()));
    Ok(())
}

#[test]
fn audit_lists_ambient_globals_alongside_declarations() -> TestResult {
    let root = single("name = \"probe\"\n", r#"return "x""#)?;
    let registry = registry().with_setup(|host| {
        host.ambient("HOST_VERSION", Value::Str("1.0".to_string()));
        Ok(())
    });

    let audit = registry.audit(root.path())?;
    // Ambient values reach a plugin whether or not it declared anything.
    assert_eq!(audit.ambient, ["HOST_VERSION"]);
    Ok(())
}

/// Reload must consult the policy once per declared capability, not twice.
///
/// The decision was implemented in two places: the backend's `grant_capabilities`,
/// which runs it to decide what to *bind*, and `Registry::evaluate_policy`, which ran
/// it again to decide what to *record*. `reload` called `reload_plugin` (the first)
/// and then `evaluate_policy` (the second), discarding the grants the backend had
/// just computed. So `granted` was derived one way on `load_dir` and another on
/// `reload`, and a policy holding a counter, a clock or an audit log saw every
/// reloaded capability twice.
///
/// The two also disagreed on the edges — a missing provider and a `Deny` were a hard
/// failure for the backend and a silent skip for the registry — which is latent while
/// `reload` only reaches the second after the first has succeeded, and stops being
/// latent the moment anything else calls it. Its own documentation offers it for
/// auditing.
#[test]
fn reload_consults_the_policy_once_per_capability() -> TestResult {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let decisions = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&decisions);

    let root = single(
        "name = \"probe\"\n\n[capabilities.log]\n",
        r#"return log(input)"#,
    )?;
    let mut registry = offering().with_policy(move |request: &CapabilityRequest| {
        counted.fetch_add(1, Ordering::Relaxed);
        if request.capability == "log" {
            Decision::Grant
        } else {
            Decision::Deny("not offered".to_string())
        }
    });

    registry.load_dir(root.path())?;
    let after_load = decisions.load(Ordering::Relaxed);
    assert_eq!(
        after_load, 1,
        "one declared capability should mean one decision at load"
    );

    registry.reload("probe")?;
    let during_reload = decisions.load(Ordering::Relaxed) - after_load;
    assert_eq!(
        during_reload, 1,
        "reload should ask the policy once, not once to bind and again to record"
    );

    // And the recorded list must still be right.
    let plugin = registry.get("probe").ok_or("probe should be loaded")?;
    assert_eq!(
        plugin.granted_capabilities().collect::<Vec<_>>(),
        vec!["log"]
    );
    assert_eq!(run(&registry, "hi")?, "logged: hi");
    Ok(())
}

/// `evaluate_policy` must refuse what loading would refuse.
///
/// It was a second implementation of the decision and disagreed with the backend's
/// on every edge: a capability the host does not offer, and a non-optional `Deny`,
/// were a hard refusal when loading and a silent skip here. So the call its own
/// documentation recommends making *before* `load` would report a plugin as fine and
/// then `load` would reject it outright. Both now run
/// `stanchion_abi::approve_capabilities`.
#[test]
fn evaluate_policy_agrees_with_loading() -> TestResult {
    use stanchion::abi::Manifest;
    use stanchion::abi::signature::Signer;

    // A capability the host does not offer at all.
    let unoffered: Manifest = toml::from_str("name = \"probe\"\n\n[capabilities.telepathy]\n")?;
    // One it offers but policy denies.
    let denied: Manifest = toml::from_str("name = \"probe\"\n\n[capabilities.log]\n")?;
    // One it offers and policy grants.
    let granted: Manifest = toml::from_str("name = \"probe\"\n\n[capabilities.log]\n")?;

    let deny_all = offering().with_policy(Rules::deny_all());
    let allow_log = offering().with_policy(Rules::deny_all().allow("log"));

    assert!(
        deny_all.evaluate_policy(&unoffered, &Signer::Unsigned).is_err(),
        "a capability the host does not offer must be refused, not skipped"
    );
    assert!(
        deny_all.evaluate_policy(&denied, &Signer::Unsigned).is_err(),
        "a non-optional denial must be refused, not skipped"
    );
    assert_eq!(
        allow_log.evaluate_policy(&granted, &Signer::Unsigned)?,
        vec!["log".to_string()]
    );

    // And the same three manifests, through a real load, reach the same verdicts.
    let root = single(
        "name = \"probe\"\n\n[capabilities.telepathy]\n",
        r#"return "unreachable""#,
    )?;
    let mut registry = offering().with_policy(Rules::deny_all());
    let report = registry.load_dir(root.path())?;
    assert!(
        !report.is_clean(),
        "loading a plugin asking for an unoffered capability must fail"
    );
    Ok(())
}

/// An `optional` capability is the one case where a refusal is a skip, in both.
#[test]
fn an_optional_capability_is_skipped_not_refused() -> TestResult {
    use stanchion::abi::Manifest;
    use stanchion::abi::signature::Signer;

    let manifest: Manifest = toml::from_str(
        "name = \"probe\"\n\n[capabilities.telepathy]\noptional = true\n",
    )?;
    let registry = offering().with_policy(Rules::deny_all());
    assert_eq!(
        registry.evaluate_policy(&manifest, &Signer::Unsigned)?,
        Vec::<String>::new()
    );
    Ok(())
}

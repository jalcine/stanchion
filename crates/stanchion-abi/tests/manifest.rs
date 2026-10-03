//! Tests for the manifest types.

use semver::VersionReq;
use stanchion_abi::manifest::{DependencySpec, DetailedDependency, Manifest, PluginType};

/// Fallible tests read better than `unwrap` and keep the workspace deny on
/// panicking helpers intact.
type Boxed = Box<dyn std::error::Error + Send + Sync>;
type Fallible<T> = std::result::Result<T, Boxed>;

/// A minimal valid Lua manifest, for tests that care about one field.
///
/// `Manifest` has no `Default` — deliberately: `name` has no sensible one — so every
/// test that builds one listed all ten fields, which meant five near-identical
/// literals here and a new field breaking all of them at once.
fn lua_manifest() -> Manifest {
    Manifest {
        name: "test".to_string(),
        version: None,
        plugin_type: PluginType::Lua,
        entry: "init.lua".to_string(),
        dependencies: Default::default(),
        capabilities: Default::default(),
        rocks: Default::default(),
        config: Default::default(),
        budget: None,
        dir: std::path::PathBuf::from("/tmp/test"),
    }
}

#[test]
fn lua_plugin_defaults() {
    let manifest = lua_manifest();

    assert_eq!(manifest.plugin_type, PluginType::Lua);
    assert!(manifest.validate().is_ok());
}

#[test]
fn wasm_plugin_defaults() -> Fallible<()> {
    let mut manifest = Manifest {
        plugin_type: PluginType::Wasm,
        entry: "init.wasm".to_string(),
        ..lua_manifest()
    };

    assert_eq!(manifest.plugin_type, PluginType::Wasm);
    assert!(manifest.validate().is_ok());

    // Wrong extension for WASM
    manifest.entry = "init.lua".to_string();
    let err = manifest
        .validate()
        .err()
        .ok_or("a lua entry must fail a wasm manifest")?;
    assert!(err.contains("wasm"));
    assert!(err.contains(".lua"));
    Ok(())
}

#[test]
fn lua_wrong_entry_is_rejected() -> Fallible<()> {
    let manifest = Manifest {
        name: "test".to_string(),
        version: None,
        plugin_type: PluginType::Lua,
        entry: "init.wasm".to_string(),
        dependencies: Default::default(),
        capabilities: Default::default(),
        rocks: Default::default(),
        config: Default::default(),
        budget: None,
        dir: std::path::PathBuf::from("/tmp/test"),
    };

    let err = manifest
        .validate()
        .err()
        .ok_or("a wasm entry must fail a lua manifest")?;
    assert!(err.contains("Lua plugin entry"));
    assert!(err.contains(".lua"));
    Ok(())
}

#[test]
fn effective_version_defaults_to_zero() {
    let manifest = lua_manifest();

    assert_eq!(manifest.effective_version(), semver::Version::new(0, 0, 0));
}

#[test]
fn effective_version_uses_declared_version() {
    let manifest = Manifest {
        version: Some(semver::Version::new(1, 2, 3)),
        ..lua_manifest()
    };

    assert_eq!(manifest.effective_version(), semver::Version::new(1, 2, 3));
}

#[test]
fn entry_path_joins_dir_and_entry() {
    let manifest = lua_manifest();

    assert_eq!(
        manifest.entry_path(),
        std::path::PathBuf::from("/tmp/test/init.lua")
    );
}

#[test]
fn dependency_spec_requirement_is_not_optional() -> Fallible<()> {
    let dep = DependencySpec::Requirement(VersionReq::parse("^1.0")?);
    assert!(!dep.is_optional());
    Ok(())
}

#[test]
fn detailed_dependency_can_be_optional() -> Fallible<()> {
    let dep = DependencySpec::Detailed(DetailedDependency {
        version: VersionReq::parse("^1.0")?,
        optional: true,
    });
    assert!(dep.is_optional());

    let dep = DependencySpec::Detailed(DetailedDependency {
        version: VersionReq::parse("^1.0")?,
        optional: false,
    });
    assert!(!dep.is_optional());
    Ok(())
}

#[test]
fn dependency_spec_requirement_access() -> Fallible<()> {
    let dep = DependencySpec::Requirement(VersionReq::parse("^1.0")?);
    let req = dep.requirement();
    assert_eq!(req.to_string(), "^1.0");
    Ok(())
}

#[test]
fn detailed_dependency_requirement_access() -> Fallible<()> {
    let dep = DependencySpec::Detailed(DetailedDependency {
        version: VersionReq::parse("^2.0")?,
        optional: false,
    });
    let req = dep.requirement();
    assert_eq!(req.to_string(), "^2.0");
    Ok(())
}

#[test]
fn manifest_serde_defaults() -> Fallible<()> {
    // Minimal manifest deserializes with defaults
    let toml_str = r#"
name = "greeter"
"#;
    let manifest: Manifest = toml::from_str(toml_str)?;
    assert_eq!(manifest.name, "greeter");
    assert_eq!(manifest.plugin_type, PluginType::Lua);
    assert_eq!(manifest.entry, "init.lua");
    Ok(())
}

#[test]
fn manifest_serde_explicit_fields() -> Fallible<()> {
    let toml_str = r#"
name = "greeter"
version = "1.2.0"
plugin_type = "lua"
entry = "main.lua"

[config]
greeting = "hello"

[dependencies]
formatter = "^1.0"
"#;
    let manifest: Manifest = toml::from_str(toml_str)?;
    assert_eq!(manifest.name, "greeter");
    assert_eq!(manifest.version, Some(semver::Version::new(1, 2, 0)));
    assert_eq!(manifest.plugin_type, PluginType::Lua);
    assert_eq!(manifest.entry, "main.lua");
    assert!(manifest.dependencies.contains_key("formatter"));
    Ok(())
}

#[test]
fn manifest_serde_optional_dependency() -> Fallible<()> {
    let toml_str = r#"
name = "caller"

[dependencies.logger]
version = "^2.0"
optional = true
"#;
    let manifest: Manifest = toml::from_str(toml_str)?;
    let dep = manifest
        .dependencies
        .get("logger")
        .ok_or("the logger dependency must parse")?;
    assert!(matches!(dep, DependencySpec::Detailed(d) if d.optional));
    Ok(())
}

/// A manifest may lower its memory ceiling, not just its instruction ceiling.
///
/// `Budget` carried only `max_instructions`, so `ResourceLimits::memory_bytes` had no
/// manifest representation at all: a plugin could promise to stay inside a smaller
/// memory footprint and nothing could express it.
#[test]
fn budget_accepts_a_memory_ceiling() -> Fallible<()> {
    let manifest: Manifest = toml::from_str(
        r#"
name = "greeter"

[budget]
max_instructions = 10000000
memory_bytes = 33554432
"#,
    )?;
    let budget = manifest.budget.ok_or("expected a budget")?;
    assert_eq!(budget.max_instructions, 10_000_000);
    assert_eq!(budget.memory_bytes, Some(33_554_432));
    Ok(())
}

/// `memory_bytes` stays optional: existing manifests declare instructions only.
#[test]
fn budget_memory_ceiling_is_optional() -> Fallible<()> {
    let manifest: Manifest = toml::from_str(
        r#"
name = "greeter"

[budget]
max_instructions = 500
"#,
    )?;
    let budget = manifest.budget.ok_or("expected a budget")?;
    assert_eq!(budget.memory_bytes, None);
    Ok(())
}

/// `narrowed_by` must narrow *both* ceilings, and only downwards.
///
/// It handled `max_instructions` and silently ignored memory, so a manifest's memory
/// ceiling would have been dropped even once `Budget` could express one.
#[test]
fn narrowing_takes_the_lower_of_each_ceiling() -> Fallible<()> {
    use stanchion_abi::ResourceLimits;

    let host = ResourceLimits {
        memory_bytes: Some(64 * 1024 * 1024),
        max_instructions: Some(10_000_000),
    };
    let manifest: Manifest = toml::from_str(
        r#"
name = "greeter"

[budget]
max_instructions = 1000
memory_bytes = 1048576
"#,
    )?;

    let narrowed = host.narrowed_by(manifest.budget.as_ref());
    assert_eq!(narrowed.max_instructions, Some(1_000));
    assert_eq!(narrowed.memory_bytes, Some(1_048_576));

    // A manifest asking for *more* than the host allows gets the host's number.
    let greedy: Manifest = toml::from_str(
        r#"
name = "greedy"

[budget]
max_instructions = 999999999
memory_bytes = 999999999
"#,
    )?;
    let clamped = host.narrowed_by(greedy.budget.as_ref());
    assert_eq!(clamped.max_instructions, Some(10_000_000));
    assert_eq!(clamped.memory_bytes, Some(64 * 1024 * 1024));
    Ok(())
}

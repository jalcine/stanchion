//! Tests for the manifest types.

use semver::VersionReq;
use stanchion_abi::manifest::{DependencySpec, DetailedDependency, Manifest, PluginType};

/// Fallible tests read better than `unwrap` and keep the workspace deny on
/// panicking helpers intact.
type Boxed = Box<dyn std::error::Error + Send + Sync>;
type Fallible<T> = std::result::Result<T, Boxed>;

#[test]
fn lua_plugin_defaults() {
    let manifest = Manifest {
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
    };

    assert_eq!(manifest.plugin_type, PluginType::Lua);
    assert!(manifest.validate().is_ok());
}

#[test]
fn wasm_plugin_defaults() -> Fallible<()> {
    let mut manifest = Manifest {
        name: "test".to_string(),
        version: None,
        plugin_type: PluginType::Wasm,
        entry: "init.wasm".to_string(),
        dependencies: Default::default(),
        capabilities: Default::default(),
        rocks: Default::default(),
        config: Default::default(),
        budget: None,
        dir: std::path::PathBuf::from("/tmp/test"),
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
    let manifest = Manifest {
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
    };

    assert_eq!(manifest.effective_version(), semver::Version::new(0, 0, 0));
}

#[test]
fn effective_version_uses_declared_version() {
    let manifest = Manifest {
        name: "test".to_string(),
        version: Some(semver::Version::new(1, 2, 3)),
        plugin_type: PluginType::Lua,
        entry: "init.lua".to_string(),
        dependencies: Default::default(),
        capabilities: Default::default(),
        rocks: Default::default(),
        config: Default::default(),
        budget: None,
        dir: std::path::PathBuf::from("/tmp/test"),
    };

    assert_eq!(manifest.effective_version(), semver::Version::new(1, 2, 3));
}

#[test]
fn entry_path_joins_dir_and_entry() {
    let manifest = Manifest {
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
    };

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

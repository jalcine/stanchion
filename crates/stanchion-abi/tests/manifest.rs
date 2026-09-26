//! Tests for the manifest types.

use stanchion_abi::manifest::{DependencySpec, DetailedDependency, Manifest, PluginType};
use semver::VersionReq;

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
fn wasm_plugin_defaults() {
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
    let err = manifest.validate().unwrap_err();
    assert!(err.contains("wasm"));
    assert!(err.contains(".lua"));
}

#[test]
fn lua_wrong_entry_is_rejected() {
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

    let err = manifest.validate().unwrap_err();
    assert!(err.contains("Lua plugin entry"));
    assert!(err.contains(".lua"));
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

    assert_eq!(manifest.entry_path(), std::path::PathBuf::from("/tmp/test/init.lua"));
}

#[test]
fn dependency_spec_requirement_is_not_optional() {
    let dep = DependencySpec::Requirement(VersionReq::parse("^1.0").unwrap());
    assert!(!dep.is_optional());
}

#[test]
fn detailed_dependency_can_be_optional() {
    let dep = DependencySpec::Detailed(DetailedDependency {
        version: VersionReq::parse("^1.0").unwrap(),
        optional: true,
    });
    assert!(dep.is_optional());

    let dep = DependencySpec::Detailed(DetailedDependency {
        version: VersionReq::parse("^1.0").unwrap(),
        optional: false,
    });
    assert!(!dep.is_optional());
}

#[test]
fn dependency_spec_requirement_access() {
    let dep = DependencySpec::Requirement(VersionReq::parse("^1.0").unwrap());
    let req = dep.requirement();
    assert_eq!(req.to_string(), "^1.0");
}

#[test]
fn detailed_dependency_requirement_access() {
    let dep = DependencySpec::Detailed(DetailedDependency {
        version: VersionReq::parse("^2.0").unwrap(),
        optional: false,
    });
    let req = dep.requirement();
    assert_eq!(req.to_string(), "^2.0");
}

#[test]
fn manifest_serde_defaults() {
    // Minimal manifest deserializes with defaults
    let toml_str = r#"
name = "greeter"
"#;
    let manifest: Manifest = toml::from_str(toml_str).unwrap();
    assert_eq!(manifest.name, "greeter");
    assert_eq!(manifest.plugin_type, PluginType::Lua);
    assert_eq!(manifest.entry, "init.lua");
}

#[test]
fn manifest_serde_explicit_fields() {
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
    let manifest: Manifest = toml::from_str(toml_str).unwrap();
    assert_eq!(manifest.name, "greeter");
    assert_eq!(manifest.version, Some(semver::Version::new(1, 2, 0)));
    assert_eq!(manifest.plugin_type, PluginType::Lua);
    assert_eq!(manifest.entry, "main.lua");
    assert!(manifest.dependencies.contains_key("formatter"));
}

#[test]
fn manifest_serde_optional_dependency() {
    let toml_str = r#"
name = "caller"

[dependencies.logger]
version = "^2.0"
optional = true
"#;
    let manifest: Manifest = toml::from_str(toml_str).unwrap();
    let dep = manifest.dependencies.get("logger").unwrap();
    assert!(matches!(dep, DependencySpec::Detailed(d) if d.optional));
}
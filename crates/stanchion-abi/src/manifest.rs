//! Plugin manifests: the static declaration a plugin ships.
//!
//! Discovery and dependency resolution live in `stanchion-registry` (they report
//! into that crate's richer error types); the manifest *types* live here so a
//! backend such as WASM can read one without linking Lua.

use std::path::{Path, PathBuf};

use semver::{Version, VersionReq};
use serde::Deserialize;

/// File each plugin directory must contain to be discovered.
pub const MANIFEST_FILE: &str = "plugin.toml";

fn default_entry() -> String {
    "init.lua".to_string()
}

/// Which runtime backend a plugin uses.
#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PluginType {
    /// A Lua plugin (the default), loaded via `mlua`.
    #[default]
    Lua,
    /// A WASM plugin, loaded via `wasmtime`.
    Wasm,
}

/// A plugin's `plugin.toml`.
///
/// ```toml
/// name = "greeter"
/// version = "1.2.0"
/// plugin_type = "lua"
/// entry = "init.lua"
///
/// [dependencies]
/// formatter = "^1.0"
/// logger = { version = "^2.0", optional = true }
///
/// [config]
/// greeting = "hello"
/// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Unique plugin name. Dispatch, reload and dependencies address plugins by this.
    pub name: String,
    /// Semver version other plugins match their requirements against.
    ///
    /// A plugin without one is treated as `0.0.0`, so it satisfies only `*`.
    #[serde(default)]
    pub version: Option<Version>,
    /// Which runtime backend this plugin uses.
    #[serde(default)]
    pub plugin_type: PluginType,
    /// File to evaluate, relative to the plugin directory.
    /// For Lua plugins this is a `.lua` file; for WASM plugins, a `.wasm` binary.
    #[serde(default = "default_entry")]
    pub entry: String,
    /// Plugins this one is wired to, by name.
    #[serde(default)]
    pub dependencies: std::collections::BTreeMap<String, DependencySpec>,
    /// Capabilities this plugin requests, as name to parameters.
    ///
    /// The reserved `optional` key is stripped by the registry; everything else is
    /// passed to the host's provider once policy has approved it.
    #[serde(default)]
    pub capabilities: std::collections::BTreeMap<String, toml::Table>,
    /// LuaRocks packages this plugin needs, as name to requirement.
    ///
    /// Always accepted so manifests stay portable, but only acted on with the
    /// `luarocks` feature and a configured tree.
    #[serde(default)]
    pub rocks: std::collections::BTreeMap<String, String>,
    /// Passed to the plugin's constructor as a Lua table.
    ///
    /// Also holds the optional `[budget]` table when the manifest carries one:
    /// `budget.max_instructions` is enforced at each call boundary for
    /// runtimes that support it (WASM, Lua sandbox).
    #[serde(default)]
    pub config: toml::Table,
    /// Optional per-plugin instruction/memory budget.
    ///
    /// Declared as `budget.max_instructions` in `plugin.toml`. Parsed into
    /// [`Manifest::budget`] at load time and enforced at call boundaries.
    #[serde(default)]
    pub budget: Option<Budget>,
    /// Directory the manifest was read from. Filled in by discovery.
    #[serde(skip)]
    pub dir: PathBuf,
}

/// Static budget declared in a plugin manifest.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Budget {
    /// Hard cap on VM instructions per plugin call.
    pub max_instructions: u64,
}

/// A dependency entry: either a bare requirement or the table form.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum DependencySpec {
    /// `formatter = "^1.0"`
    Requirement(VersionReq),
    /// `formatter = { version = "^1.0", optional = true }`
    Detailed(DetailedDependency),
}

/// The table form of a dependency.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DetailedDependency {
    /// Semver requirement the dependency's version must satisfy.
    pub version: VersionReq,
    /// When true, an absent dependency leaves a `nil` slot instead of failing.
    #[serde(default)]
    pub optional: bool,
}

impl DependencySpec {
    /// The semver requirement this dependency must satisfy.
    pub fn requirement(&self) -> &VersionReq {
        match self {
            DependencySpec::Requirement(requirement) => requirement,
            DependencySpec::Detailed(detailed) => &detailed.version,
        }
    }

    /// Whether the plugin still loads when this dependency is absent.
    pub fn is_optional(&self) -> bool {
        match self {
            DependencySpec::Requirement(_) => false,
            DependencySpec::Detailed(detailed) => detailed.optional,
        }
    }
}

/// Confirms a plugin name is a safe, portable identifier.
///
/// The name flows into filesystem paths (`root/<name>`, staging directories), log
/// lines, remote-protocol messages and error strings, so it is held to a strict
/// grammar: 1–128 characters of ASCII letters, digits, `-`, `_` and `.`, and never `.`
/// or `..`. This matches the distribution index's grammar so a name is validated the
/// same way whether it came from a manifest or an index lookup. See #42.
pub fn validate_name(name: &str) -> Result<(), String> {
    let acceptable = !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        && name != "."
        && name != "..";
    if acceptable {
        Ok(())
    } else {
        Err(format!(
            "`{name}` is not a usable plugin name: use 1-128 ASCII letters, digits, `-`, `_` and `.`"
        ))
    }
}

impl Manifest {
    /// Validates that the manifest's entry file matches its plugin type and names a
    /// file *inside* the plugin directory.
    ///
    /// The entry must be a relative path with no `..` and no root/prefix component, so
    /// `entry` cannot point at `../elsewhere/x.wasm` or an absolute path that the
    /// plugin's digest never covers. Enforced for every backend. See #35.
    pub fn validate(&self) -> Result<(), String> {
        validate_name(&self.name)?;
        self.validate_entry_path()?;
        match &self.plugin_type {
            PluginType::Lua if !self.entry.ends_with(".lua") => Err(format!(
                "Lua plugin entry must end with '.lua', got '{}'",
                self.entry
            )),
            PluginType::Wasm if !self.entry.ends_with(".wasm") => Err(format!(
                "WASM plugin entry must end with '.wasm', got '{}'",
                self.entry
            )),
            _ => Ok(()),
        }
    }

    /// Confirms `entry` is a relative path confined to the plugin directory.
    fn validate_entry_path(&self) -> Result<(), String> {
        use std::path::Component;
        if self.entry.is_empty() {
            return Err("plugin entry must not be empty".to_string());
        }
        let path = Path::new(&self.entry);
        for component in path.components() {
            match component {
                Component::Normal(_) | Component::CurDir => {}
                Component::ParentDir => {
                    return Err(format!(
                        "plugin entry '{}' must not contain '..'",
                        self.entry
                    ));
                }
                Component::RootDir | Component::Prefix(_) => {
                    return Err(format!(
                        "plugin entry '{}' must be a relative path inside the plugin",
                        self.entry
                    ));
                }
            }
        }
        Ok(())
    }

    /// Absolute path of the plugin's entry chunk.
    pub fn entry_path(&self) -> PathBuf {
        self.dir.join(&self.entry)
    }

    /// The version other plugins match against; absent means `0.0.0`.
    pub fn effective_version(&self) -> Version {
        self.version
            .clone()
            .unwrap_or_else(|| Version::new(0, 0, 0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(plugin_type: &str, entry: &str) -> Manifest {
        let raw = format!("name = \"p\"\nplugin_type = \"{plugin_type}\"\nentry = \"{entry}\"\n");
        toml::from_str(&raw).expect("manifest parses")
    }

    #[test]
    fn accepts_a_relative_entry_inside_the_plugin() {
        assert!(manifest("lua", "init.lua").validate().is_ok());
        assert!(manifest("lua", "src/init.lua").validate().is_ok());
        assert!(manifest("wasm", "plugin.wasm").validate().is_ok());
        assert!(manifest("wasm", "./build/plugin.wasm").validate().is_ok());
    }

    #[test]
    fn accepts_ordinary_plugin_names() {
        for name in ["probe", "my-plugin", "weather_v2", "a.b.c", "30log"] {
            assert!(validate_name(name).is_ok(), "{name} should be valid");
        }
    }

    #[test]
    fn rejects_unsafe_plugin_names() {
        for name in ["", ".", "..", "a/b", "../evil", "has space", "a:b", &"x".repeat(129)] {
            assert!(validate_name(name).is_err(), "`{name}` must be refused");
        }
        // A manifest carrying a bad name fails validate() as a whole.
        assert!(manifest("lua", "init.lua")
            .validate()
            .is_ok());
        let mut bad = manifest("lua", "init.lua");
        bad.name = "../evil".to_string();
        assert!(bad.validate().is_err(), "a path-like name must be refused");
    }

    #[test]
    fn rejects_entries_that_escape_the_plugin_directory() {
        // Traversal, absolute paths and empty entries never reach a backend, so the
        // digest cannot be sidestepped by pointing `entry` outside the plugin (#35).
        for entry in ["../elsewhere/x.wasm", "../../etc/init.lua", ""] {
            assert!(
                manifest("wasm", entry).validate().is_err(),
                "`{entry}` must be refused"
            );
        }
        // Absolute paths differ by platform; build one directly.
        let mut m = manifest("wasm", "plugin.wasm");
        m.entry = if cfg!(windows) {
            "C:\\evil\\x.wasm".to_string()
        } else {
            "/etc/evil.wasm".to_string()
        };
        assert!(m.validate().is_err(), "absolute entry must be refused");
    }
}

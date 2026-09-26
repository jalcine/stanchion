//! Declarative configuration for a registry built by something other than Rust code.
//!
//! The builder API is the right way to configure a registry from Rust: it takes
//! closures, and a closure is the most precise way to say what a capability provides.
//! Neither an out-of-process host nor a foreign-language binding can be handed one —
//! the host binary is compiled before the plugins exist, and a Python caller has no
//! `impl Fn(&Lua, &Grant)` to give. Both need the same thing instead: a description
//! of the policy in data, read from somewhere the plugins cannot write.
//!
//! This module is that description. [`HostConfig`] is what `plugin-host` reads from
//! its TOML file and what a binding builds from its own arguments, so the two
//! transports are configured in one vocabulary rather than two that drift.

use std::path::PathBuf;

use mlua::StdLib;
use serde::Deserialize;

use stanchion_lua::sandbox::Sandbox;

/// How a host runs plugins: what they may reach, and what they cost.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostConfig {
    /// Directory to load plugins from when the caller does not name one.
    #[serde(default)]
    pub plugins: Option<PathBuf>,
    /// Lua state policy for each plugin.
    #[serde(default)]
    pub sandbox: SandboxConfig,
    /// Capability names the host will grant.
    #[serde(default)]
    pub capabilities: CapabilityConfig,
    /// Signature requirements.
    #[serde(default)]
    pub signatures: SignatureConfig,
}

/// Standard libraries and resource limits for plugin states.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxConfig {
    /// Library names: `string`, `table`, `math`, `coroutine`, `package`, `io`, `os`.
    #[serde(default)]
    pub libs: Option<Vec<String>>,
    /// Globals to unbind after the libraries load, by dotted path.
    #[serde(default)]
    pub deny: Option<Vec<String>>,
    /// Memory ceiling per plugin, in bytes.
    #[serde(default)]
    pub memory_limit: Option<usize>,
    /// Instruction ceiling per call.
    #[serde(default)]
    pub instruction_limit: Option<u64>,
    /// Whether a plugin's `pcall` may swallow a host panic.
    #[serde(default)]
    pub catch_rust_panics: Option<bool>,
    /// Whether plugins share one state.
    ///
    /// Defaults to false. A caller who reached for this module is holding plugins at
    /// arm's length already; sharing a state would undo the per-plugin memory and
    /// instruction limits below, which are properties of a [`mlua::Lua`] and cannot
    /// be enforced any other way.
    #[serde(default)]
    pub shared: bool,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        SandboxConfig {
            libs: None,
            deny: None,
            memory_limit: Some(64 * 1024 * 1024),
            instruction_limit: Some(50_000_000),
            shared: false,
            catch_rust_panics: None,
        }
    }
}

/// Which capabilities the host grants.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityConfig {
    /// Capability names to grant as requested. Anything else is denied.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Capabilities the host answers itself rather than the registry providing.
    ///
    /// Each becomes a Lua function that calls back out: to the process that launched
    /// `plugin-host`, or to the foreign-language provider a binding registered.
    /// Listing one here also grants it.
    #[serde(default)]
    pub callbacks: Vec<String>,
}

/// Signature policy.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignatureConfig {
    /// Refuse any plugin without a verified signature.
    #[serde(default)]
    pub required: bool,
}

impl SandboxConfig {
    /// Translates the configured library names into a [`StdLib`] set.
    ///
    /// An unknown name is an error rather than a silent omission: quietly dropping a
    /// library a plugin needs produces a confusing runtime failure instead of a clear
    /// configuration one.
    pub fn to_sandbox(&self) -> Result<Sandbox, String> {
        let mut sandbox = Sandbox::restricted();

        if let Some(names) = &self.libs {
            let mut libs = StdLib::NONE;
            for name in names {
                libs |= match name.as_str() {
                    "string" => StdLib::STRING,
                    "table" => StdLib::TABLE,
                    "math" => StdLib::MATH,
                    "coroutine" => StdLib::COROUTINE,
                    "package" => StdLib::PACKAGE,
                    "io" => StdLib::IO,
                    "os" => StdLib::OS,
                    "debug" => StdLib::DEBUG,
                    other => return Err(format!("unknown standard library `{other}`")),
                };
            }
            sandbox = sandbox.libs(libs);
        }

        if let Some(deny) = &self.deny {
            sandbox = sandbox.deny(deny.clone());
        }
        if let Some(bytes) = self.memory_limit {
            sandbox = sandbox.memory_limit(bytes);
        }
        if let Some(on) = self.catch_rust_panics {
            sandbox = sandbox.catch_rust_panics(on);
        }
        if let Some(instructions) = self.instruction_limit {
            sandbox = sandbox.instruction_limit(instructions);
        }
        Ok(sandbox)
    }
}

/// Reads a [`HostConfig`] from a TOML file.
pub fn load_config(path: &std::path::Path) -> Result<HostConfig, String> {
    let source = std::fs::read_to_string(path)
        .map_err(|err| format!("reading `{}`: {err}", path.display()))?;
    toml::from_str(&source).map_err(|err| format!("parsing `{}`: {err}", path.display()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn unknown_library_is_rejected() {
        let config = SandboxConfig {
            libs: Some(vec!["string".to_string(), "sorcery".to_string()]),
            ..SandboxConfig::default()
        };
        assert_eq!(
            config.to_sandbox().err(),
            Some("unknown standard library `sorcery`".to_string())
        );
    }

    #[test]
    fn defaults_cap_memory_and_instructions() {
        let config = SandboxConfig::default();
        assert!(config.memory_limit.is_some());
        assert!(config.instruction_limit.is_some());
        assert!(!config.shared);
    }

    #[test]
    fn an_empty_document_is_a_valid_config() {
        let config: HostConfig = toml::from_str("").expect("empty config");
        assert!(config.capabilities.allow.is_empty());
        assert!(!config.signatures.required);
    }

    /// Every field is optional, independently. A host that forwards one capability
    /// and grants nothing outright writes only `callbacks`, and a host that grants
    /// without forwarding writes only `allow`; neither should have to name the other.
    #[test]
    fn each_capability_list_stands_on_its_own() {
        let forwarding: HostConfig =
            toml::from_str("[capabilities]\ncallbacks = [\"kv\"]\n").expect("callbacks only");
        assert_eq!(forwarding.capabilities.callbacks, vec!["kv".to_string()]);
        assert!(forwarding.capabilities.allow.is_empty());

        let granting: HostConfig =
            toml::from_str("[capabilities]\nallow = [\"log\"]\n").expect("allow only");
        assert_eq!(granting.capabilities.allow, vec!["log".to_string()]);
        assert!(granting.capabilities.callbacks.is_empty());
    }
}

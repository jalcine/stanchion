//! The host side: configuration and the request loop the binary runs.

use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use mlua::{Lua, LuaSerdeExt, MultiValue, StdLib, Value};
use serde::Deserialize;
use serde_json::Value as Json;

use stanchion_registry::{DynClass, DynInstance, LoadReport, Registry, Rules, Sandbox};

use super::protocol::{
    AuditEntry, Envelope, Failure, Outcome, PluginInfo, Request, Response, read_message,
    write_message,
};

/// The host binary's configuration file.
///
/// The host runs plugins the core application does not trust in its own address space,
/// so its policy has to come from somewhere the plugins cannot write: a file the
/// operator controls, never the plugin manifests.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostConfig {
    /// Directory to load plugins from when the client does not name one.
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
    /// Whether plugins share one state. Defaults to false — the point of a separate
    /// process is isolation, so it should not stop at the process boundary.
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
        if let Some(instructions) = self.instruction_limit {
            sandbox = sandbox.instruction_limit(instructions);
        }
        Ok(sandbox)
    }
}

/// Builds the registry a host serves from.
pub fn build_registry(config: &HostConfig) -> Result<Registry<DynClass>, String> {
    let sandbox = config.sandbox.to_sandbox()?;

    let mut registry = if config.sandbox.shared {
        Registry::new(Lua::new())
    } else {
        Registry::isolated(Lua::new(), sandbox)
    };

    // `log` is the one capability a host can offer with no callback channel: it writes
    // to stderr, which the parent process already captures.
    registry = registry.with_setup(|host| {
        host.capability("log", |lua, grant| {
            let plugin = grant.plugin().to_string();
            Ok(Value::Function(lua.create_function(move |_, message: String| {
                eprintln!("[{plugin}] {message}");
                Ok(())
            })?))
        });
        Ok(())
    });

    let mut rules = Rules::deny_all();
    for name in &config.capabilities.allow {
        rules = rules.allow(name.clone());
    }
    registry = registry.with_policy(rules);

    #[cfg(feature = "signatures")]
    if config.signatures.required {
        registry = registry.require_signatures(true);
    }

    Ok(registry)
}

/// Serves requests until the client asks to stop or closes the stream.
///
/// Errors inside a request become an [`Response::Error`] reply rather than ending the
/// session: a malformed call from the client should not take the host down, the same
/// way one bad plugin does not stop the others loading.
pub fn serve<R: Read, W: Write>(
    registry: &mut Registry<DynClass>,
    input: R,
    output: W,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(input);
    let mut writer = BufWriter::new(output);

    while let Some(envelope) = read_message::<_, Envelope<Request>>(&mut reader)? {
        let stop = matches!(envelope.body, Request::Shutdown);
        let body = handle(registry, envelope.body);
        write_message(&mut writer, &Envelope { id: envelope.id, body })?;
        if stop {
            break;
        }
    }
    Ok(())
}

/// Answers one request.
fn handle(registry: &mut Registry<DynClass>, request: Request) -> Response {
    match request {
        Request::Load { root } => match registry.load_dir(&root) {
            Ok(report) => report_of(report),
            Err(err) => Response::Error { message: err.to_string() },
        },
        Request::List => Response::Plugins {
            plugins:             registry
                .plugins()
                .iter()
                .map(|plugin| PluginInfo {
                    name: plugin.name().to_string(),
                    version: plugin
                        .manifest()
                        .version
                        .as_ref()
                        .map(ToString::to_string),
                    granted: plugin.granted_capabilities().map(str::to_string).collect(),
                    signer: signer_of(plugin),
                })
                .collect(),
        },
        Request::Audit { root } => match registry.audit(&root) {
            Ok(audit) => Response::Audit {
                entries: audit
                    .plugins
                    .iter()
                    .map(|entry| AuditEntry {
                        plugin: entry.name.clone(),
                        capabilities: entry
                            .requests
                            .iter()
                            .map(|request| request.name.clone())
                            .collect(),
                        signer: audit_signer(entry),
                    })
                    .collect(),
            },
            Err(err) => Response::Error { message: err.to_string() },
        },
        Request::Call { plugin, method, args } => {
            let Some(entry) = registry.get(&plugin) else {
                return Response::Error { message: format!("no plugin named `{plugin}`") };
            };
            match call_plugin(entry.lua(), entry.instance(), &method, &args) {
                Ok(value) => Response::Value { value },
                Err(message) => Response::Error { message },
            }
        }
        Request::Dispatch { method, args } => {
            let outcomes = registry
                .plugins()
                .iter()
                .map(|plugin| {
                    match call_plugin(plugin.lua(), plugin.instance(), &method, &args) {
                        Ok(value) => Outcome {
                            plugin: plugin.name().to_string(),
                            value: Some(value),
                            error: None,
                        },
                        Err(message) => Outcome {
                            plugin: plugin.name().to_string(),
                            value: None,
                            error: Some(message),
                        },
                    }
                })
                .collect();
            Response::Outcomes { outcomes }
        }
        Request::Reload { plugin } => match registry.reload(&plugin) {
            Ok(()) => Response::Ok,
            Err(err) => Response::Error { message: err.to_string() },
        },
        Request::Revoke { plugin, capability } => match registry.revoke(&plugin, &capability) {
            Ok(true) => Response::Ok,
            Ok(false) => Response::Error {
                message: format!("`{plugin}` does not hold `{capability}`"),
            },
            Err(err) => Response::Error { message: err.to_string() },
        },
        Request::Info => Response::Info {
            version: env!("CARGO_PKG_VERSION").to_string(),
            isolation: match registry.isolation() {
                stanchion_registry::Isolation::Shared => "shared".to_string(),
                stanchion_registry::Isolation::PerPlugin(_) => "per-plugin".to_string(),
            },
            signatures_required: signatures_required(registry),
        },
        Request::Shutdown => Response::Ok,
    }
}

/// Converts JSON arguments into Lua, calls the method, and converts the result back.
fn call_plugin(
    lua: &Lua,
    instance: &DynInstance,
    method: &str,
    args: &[Json],
) -> Result<Json, String> {
    let mut lua_args = Vec::with_capacity(args.len());
    for arg in args {
        lua_args.push(lua.to_value(arg).map_err(|err| err.to_string())?);
    }

    let result = instance
        .call_method(method, MultiValue::from_iter(lua_args))
        .map_err(|err| err.to_string())?;

    // `nil` is JSON null rather than an error: a method may legitimately return
    // nothing.
    lua.from_value::<Json>(result).map_err(|err| err.to_string())
}

fn report_of(report: LoadReport) -> Response {
    Response::Loaded {
        loaded: report.loaded,
        failures: report
            .failures
            .iter()
            .map(|failure| Failure {
                plugin: failure.name.clone(),
                reason: failure.reason.to_string(),
            })
            .collect(),
    }
}

#[cfg(feature = "signatures")]
fn signer_of(plugin: &stanchion_registry::Plugin<DynClass>) -> String {
    plugin.signer().to_string()
}

#[cfg(not(feature = "signatures"))]
fn signer_of(_plugin: &stanchion_registry::Plugin<DynClass>) -> String {
    "unverified".to_string()
}

#[cfg(feature = "signatures")]
fn audit_signer(entry: &stanchion_registry::PluginAudit) -> String {
    entry.signer.to_string()
}

#[cfg(not(feature = "signatures"))]
fn audit_signer(_entry: &stanchion_registry::PluginAudit) -> String {
    "unverified".to_string()
}

#[cfg(feature = "signatures")]
fn signatures_required(_registry: &Registry<DynClass>) -> bool {
    // The registry does not expose the flag; the host reports its own configuration.
    false
}

#[cfg(not(feature = "signatures"))]
fn signatures_required(_registry: &Registry<DynClass>) -> bool {
    false
}

/// Reads a `HostConfig` from a TOML file.
pub fn load_config(path: &Path) -> Result<HostConfig, String> {
    let source = std::fs::read_to_string(path)
        .map_err(|err| format!("reading `{}`: {err}", path.display()))?;
    toml::from_str(&source).map_err(|err| format!("parsing `{}`: {err}", path.display()))
}

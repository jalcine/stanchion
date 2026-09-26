//! The host side: configuration and the request loop the binary runs.

use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};

use mlua::{Lua, LuaSerdeExt, MultiValue, Value};
use serde_json::Value as Json;

use stanchion_registry::config::HostConfig;
use stanchion_registry::{DynClass, DynInstance, Registry, Rules};

use jsonrpsee_types::{ErrorCode, ErrorObjectOwned, Id};

use crate::frame::{self, Incoming, Request, Response};

use super::protocol::{
    AuditEntry, CallParams, CallbackCall, DispatchParams, Failure, HostInfo, LoadResult, Outcome,
    PluginInfo, PluginParams, RevokeParams, RootParams, error_code, method,
};

/// Builds the registry a host serves from.
pub fn build_registry(
    config: &HostConfig,
    channel: &HostChannel,
) -> Result<Registry<DynClass>, String> {
    let sandbox = config.sandbox.to_sandbox()?;

    let mut registry = if config.sandbox.shared {
        Registry::new(Lua::new())
    } else {
        Registry::isolated(Lua::new(), sandbox)
    };

    let forwarded = config.capabilities.callbacks.clone();
    let channel = channel.clone();
    registry = registry.with_setup(move |host| {
        // `log` needs no callback channel: it writes to stderr, which the process
        // that launched the host already captures.
        host.capability("log", |lua, grant| {
            let plugin = grant.plugin().to_string();
            let lua_state = lua.lua_state().expect("Lua runtime expected");
            let lua_guard = lua_state.lock().unwrap();
            Ok(Value::Function(lua_guard.create_function(
                move |_, message: String| {
                    eprintln!("[{plugin}] {message}");
                    Ok(())
                },
            )?))
        });

        for capability in forwarded {
            let channel = channel.clone();
            host.capability(capability.clone(), move |lua, grant| {
                let channel = channel.clone();
                let capability = capability.clone();
                let plugin = grant.plugin().to_string();
                // The approved grant travels with every call so the application can
                // re-check it rather than trusting this host to have narrowed.
                let granted = serde_json::to_value(grant.params()).unwrap_or(Json::Null);

                Ok(Value::Function(lua.create_function(
                    move |lua, args: mlua::MultiValue| {
                        let mut json_args = Vec::with_capacity(args.len());
                        for arg in args {
                            json_args.push(lua.from_value::<Json>(arg)?);
                        }
                        let value = channel
                            .call_application(CallbackCall {
                                plugin: plugin.clone(),
                                capability: capability.clone(),
                                grant: granted.clone(),
                                args: json_args,
                            })
                            .map_err(mlua::Error::RuntimeError)?;
                        lua.to_value(&value)
                    },
                )?))
            });
        }
        Ok(())
    });

    let mut rules = Rules::deny_all();
    for name in config
        .capabilities
        .allow
        .iter()
        .chain(&config.capabilities.callbacks)
    {
        rules = rules.allow(name.clone());
    }
    registry = registry.with_policy(rules);

    #[cfg(feature = "signatures")]
    if config.signatures.required {
        registry = registry.require_signatures(true);
    }

    Ok(registry)
}

/// The host's end of the channel: framed JSON-RPC over a pair of pipes.
///
/// Shared, because a capability provider is a `'static` Lua closure that must reach
/// the channel long after `serve` was called. A callback made from inside Lua blocks
/// for its reply while queueing any request that arrives meanwhile, so the
/// application can keep sending while a plugin is mid-call.
#[derive(Clone)]
pub struct HostChannel {
    state: Arc<Mutex<ChannelState>>,
}

struct ChannelState {
    reader: Box<dyn BufRead + Send>,
    writer: Box<dyn Write + Send>,
    queued: VecDeque<Request>,
    next_callback: u64,
}

impl HostChannel {
    /// Wraps the host's input and output.
    pub fn new(reader: impl BufRead + Send + 'static, writer: impl Write + Send + 'static) -> Self {
        HostChannel {
            state: Arc::new(Mutex::new(ChannelState {
                reader: Box::new(reader),
                writer: Box::new(writer),
                queued: VecDeque::new(),
                next_callback: 1,
            })),
        }
    }

    /// The next request to serve: a queued one first, then whatever arrives.
    fn next_request(&self) -> std::io::Result<Option<Request>> {
        let mut state = self.lock()?;
        if let Some(queued) = state.queued.pop_front() {
            return Ok(Some(queued));
        }
        loop {
            match frame::read(&mut state.reader)? {
                None => return Ok(None),
                Some(Incoming::Request(request)) => return Ok(Some(request)),
                // A reply with nothing waiting for it is a confused peer, not a
                // reason to stop serving.
                Some(Incoming::Response(_)) => continue,
            }
        }
    }

    fn send(&self, response: Response) -> std::io::Result<()> {
        let mut state = self.lock()?;
        frame::write(&mut state.writer, &response)
    }

    /// Calls back into the application and waits for its answer.
    pub fn call_application(&self, call: CallbackCall) -> Result<Json, String> {
        let method_name = format!("{}{}", method::CAPABILITY_PREFIX, call.capability);
        let params = serde_json::to_value(&call).map_err(|err| err.to_string())?;

        let mut state = self.lock().map_err(|err| err.to_string())?;
        let id = state.next_callback;
        state.next_callback = state.next_callback.saturating_add(1);

        frame::write(&mut state.writer, &Request::new(id, method_name, params))
            .map_err(|err| format!("sending the callback: {err}"))?;
        let id = Id::Number(id);

        loop {
            match frame::read(&mut state.reader) {
                Err(err) => return Err(format!("awaiting the callback reply: {err}")),
                Ok(None) => return Err("the application closed the channel".to_string()),
                Ok(Some(Incoming::Response(response))) if response.id == id => {
                    return match (response.result, response.error) {
                        (_, Some(error)) => Err(error.message().to_string()),
                        (Some(value), None) => Ok(value),
                        (None, None) => Ok(Json::Null),
                    };
                }
                // Someone else's reply; nothing sensible to do but ignore it.
                Ok(Some(Incoming::Response(_))) => continue,
                // Keep serving what the application sends while we wait.
                Ok(Some(Incoming::Request(request))) => state.queued.push_back(request),
            }
        }
    }

    fn lock(&self) -> std::io::Result<std::sync::MutexGuard<'_, ChannelState>> {
        self.state
            .lock()
            .map_err(|_| std::io::Error::other("the host channel was poisoned by a panic"))
    }
}

/// Serves requests until the client asks to stop or closes the stream.
///
/// A failure inside one request becomes an error reply rather than ending the
/// session: a malformed call from the application should not take the host down, the
/// same way one bad plugin does not stop the others loading.
pub fn serve(registry: &mut Registry<DynClass>, channel: &HostChannel) -> std::io::Result<()> {
    while let Some(request) = channel.next_request()? {
        let stop = request.method == method::SHUTDOWN;
        let id = request.id.clone();
        let response = match handle(registry, request) {
            Ok(value) => Response::ok(id, value),
            Err(error) => Response::failed(id, error),
        };
        channel.send(response)?;
        if stop {
            break;
        }
    }
    Ok(())
}

fn failed(message: impl Into<String>) -> ErrorObjectOwned {
    frame::app_error(error_code::REQUEST_FAILED, message)
}

fn parse<T: serde::de::DeserializeOwned>(params: Option<Json>) -> Result<T, ErrorObjectOwned> {
    serde_json::from_value(params.unwrap_or(Json::Null))
        .map_err(|err| frame::error(ErrorCode::InvalidParams, err.to_string()))
}

fn encode<T: serde::Serialize>(value: &T) -> Result<Json, ErrorObjectOwned> {
    serde_json::to_value(value).map_err(|err| failed(err.to_string()))
}

/// Answers one request.
fn handle(registry: &mut Registry<DynClass>, request: Request) -> Result<Json, ErrorObjectOwned> {
    match request.method.as_str() {
        method::LOAD => {
            let RootParams { root } = parse(request.params)?;
            let report = registry
                .load_dir(&root)
                .map_err(|err| failed(err.to_string()))?;
            encode(&LoadResult {
                loaded: report.loaded,
                failures: report
                    .failures
                    .iter()
                    .map(|failure| Failure {
                        plugin: failure.name.clone(),
                        reason: failure.reason.to_string(),
                    })
                    .collect(),
            })
        }
        method::LIST => encode(
            &registry
                .plugins()
                .iter()
                .map(|plugin| PluginInfo {
                    name: plugin.name().to_string(),
                    version: plugin.manifest().version.as_ref().map(ToString::to_string),
                    granted: plugin.granted_capabilities().map(str::to_string).collect(),
                    signer: signer_of(plugin),
                })
                .collect::<Vec<_>>(),
        ),
        method::AUDIT => {
            let RootParams { root } = parse(request.params)?;
            let audit = registry
                .audit(&root)
                .map_err(|err| failed(err.to_string()))?;
            encode(
                &audit
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
                    .collect::<Vec<_>>(),
            )
        }
        method::CALL => {
            let CallParams {
                plugin,
                method,
                args,
            } = parse(request.params)?;
            let entry = registry
                .get(&plugin)
                .ok_or_else(|| failed(format!("no plugin named `{plugin}`")))?;
            call_plugin(entry.lua(), entry.instance(), &method, &args).map_err(failed)
        }
        method::DISPATCH => {
            let DispatchParams { method, args } = parse(request.params)?;
            encode(
                &registry
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
                    .collect::<Vec<_>>(),
            )
        }
        method::RELOAD => {
            let PluginParams { plugin } = parse(request.params)?;
            registry
                .reload(&plugin)
                .map_err(|err| failed(err.to_string()))?;
            Ok(Json::Null)
        }
        method::REVOKE => {
            let RevokeParams { plugin, capability } = parse(request.params)?;
            match registry.revoke(&plugin, &capability) {
                Ok(true) => Ok(Json::Null),
                Ok(false) => Err(failed(format!("`{plugin}` does not hold `{capability}`"))),
                Err(err) => Err(failed(err.to_string())),
            }
        }
        method::INFO => encode(&HostInfo {
            version: env!("CARGO_PKG_VERSION").to_string(),
            isolation: match registry.isolation() {
                stanchion_registry::Isolation::Shared => "shared".to_string(),
                stanchion_registry::Isolation::PerPlugin(_) => "per-plugin".to_string(),
                stanchion_registry::Isolation::PerGroup(_) => "per-group".to_string(),
            },
            signatures_required: false,
        }),
        method::SHUTDOWN => Ok(Json::Null),
        other => Err(frame::error(
            ErrorCode::MethodNotFound,
            format!("unknown method `{other}`"),
        )),
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
    lua.from_value::<Json>(result)
        .map_err(|err| err.to_string())
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

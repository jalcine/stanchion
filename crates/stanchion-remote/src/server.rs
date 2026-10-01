//! The host side: configuration and the request loop the binary runs.

use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};

use serde_json::Value as Json;
use stanchion_abi::{CapabilityCall, Value, sanitize_log};
use stanchion_lua::config::HostConfig;
use stanchion_registry::{Registry, Rules};

use jsonrpsee_types::{ErrorCode, ErrorObjectOwned, Id};

use crate::frame::{self, Incoming, Request, Response};

use super::protocol::{
    AuditEntry, CallParams, CallbackCall, DispatchParams, Failure, HostInfo, LoadResult, Outcome,
    PluginInfo, PluginParams, RevokeParams, RootParams, error_code, method,
};

/// Builds the registry a host serves from, plus its isolation label.
///
/// The registry drives a Lua backend; forwarded capabilities call back out
/// to the application over the channel.
pub fn build_registry(
    config: &HostConfig,
    channel: &HostChannel,
) -> Result<(Registry, &'static str), String> {
    let sandbox = config.sandbox.to_sandbox()?;
    let isolation = if config.sandbox.shared {
        "shared"
    } else {
        "per-plugin"
    };

    let mut registry = Registry::new();
    registry = registry.with_runtime(Box::new(if config.sandbox.shared {
        stanchion_lua::backend::LuaBackend::shared()
    } else {
        stanchion_lua::backend::LuaBackend::isolated(sandbox)
    }));

    let forwarded = config.capabilities.callbacks.clone();
    let channel = channel.clone();
    registry = registry.with_setup(move |host| {
        // `log` needs no callback channel: it writes to stderr, which the process
        // that launched the host already captures.
        host.capability("log", |call: &CapabilityCall| {
            let message = call
                .args
                .first()
                .and_then(|value| match value {
                    Value::Str(text) => Some(text.as_str()),
                    _ => None,
                })
                .unwrap_or("(no message)");
            eprintln!(
                "[{}] {}",
                sanitize_log(&call.plugin),
                sanitize_log(message)
            );
            Ok(Value::Nil)
        });

        for capability in forwarded {
            let channel = channel.clone();
            let capability_name = capability.clone();
            host.capability(capability, move |call: &CapabilityCall| {
                // The approved grant travels with every call so the application can
                // re-check it rather than trusting this host to have narrowed.
                let granted = value_to_json(&call.grant).unwrap_or(Json::Null);
                let mut json_args = Vec::with_capacity(call.args.len());
                for arg in &call.args {
                    json_args.push(
                        value_to_json(arg).map_err(|err| format!("converting argument: {err}"))?,
                    );
                }
                let value = channel
                    .call_application(CallbackCall {
                        plugin: call.plugin.clone(),
                        capability: capability_name.clone(),
                        grant: granted,
                        args: json_args,
                    })
                    .map_err(|err| err.to_string())?;
                json_to_value(&value).map_err(|err| err.to_string())
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

    Ok((registry, isolation))
}

/// Converts a runtime [`Value`] into JSON for the wire.
fn value_to_json(value: &Value) -> Result<Json, String> {
    match value {
        Value::Nil => Ok(Json::Null),
        Value::Bool(flag) => Ok(Json::Bool(*flag)),
        Value::Int(number) => Ok(Json::Number((*number).into())),
        Value::Float(number) => serde_json::Number::from_f64(*number)
            .map(Json::Number)
            .ok_or_else(|| format!("non-finite float `{number}` has no JSON form")),
        Value::Str(text) => Ok(Json::String(text.clone())),
        Value::List(items) => items.iter().map(value_to_json).collect(),
        Value::Map(entries) => entries
            .iter()
            .map(|(key, value)| value_to_json(value).map(|value| (key.clone(), value)))
            .collect::<Result<_, _>>()
            .map(Json::Object),
        Value::Function => Err("a capability function cannot cross the process boundary".to_string()),
    }
}

/// Converts wire JSON into a runtime [`Value`].
fn json_to_value(value: &Json) -> Result<Value, String> {
    match value {
        Json::Null => Ok(Value::Nil),
        Json::Bool(flag) => Ok(Value::Bool(*flag)),
        Json::Number(number) => number
            .as_i64()
            .map(Value::Int)
            .or_else(|| number.as_f64().map(Value::Float))
            .ok_or_else(|| format!("number `{number}` is neither an int nor a float")),
        Json::String(text) => Ok(Value::Str(text.clone())),
        Json::Array(items) => items
            .iter()
            .map(json_to_value)
            .collect::<Result<Vec<_>, _>>()
            .map(Value::List),
        Json::Object(entries) => entries
            .iter()
            .map(|(key, value)| json_to_value(value).map(|value| (key.clone(), value)))
            .collect::<Result<_, _>>()
            .map(Value::Map),
    }
}

/// The host's end of the channel: framed JSON-RPC over a pair of pipes.
///
/// Shared, because a capability provider is a `'static` closure that must reach
/// the channel long after `serve` was called. A callback made from inside a
/// plugin blocks for its reply while queueing any request that arrives
/// meanwhile, so the application can keep sending while a plugin is mid-call.
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
pub fn serve(
    registry: &mut Registry,
    channel: &HostChannel,
    isolation: &str,
) -> std::io::Result<()> {
    while let Some(request) = channel.next_request()? {
        let stop = request.method == method::SHUTDOWN;
        let id = request.id.clone();
        let response = match handle(registry, isolation, request) {
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
fn handle(registry: &mut Registry, isolation: &str, request: Request) -> Result<Json, ErrorObjectOwned> {
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
                            .map(|request| request.capability.clone())
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
            let values: Vec<Value> = args
                .iter()
                .map(json_to_value)
                .collect::<Result<_, _>>()
                .map_err(failed)?;
            registry
                .call(&plugin, &method, &values)
                .map_err(|err| failed(err.to_string()))
                .and_then(|value| value_to_json(&value).map_err(failed))
        }
        method::DISPATCH => {
            let DispatchParams { method, args } = parse(request.params)?;
            let values: Vec<Value> = args
                .iter()
                .map(json_to_value)
                .collect::<Result<_, _>>()
                .map_err(failed)?;
            encode(
                &registry
                    .dispatch(&method, &values)
                    .into_iter()
                    .map(|outcome| {
                        let (value, error) = match outcome.value {
                            Some(value) => match value_to_json(&value) {
                                Ok(json) => (Some(json), None),
                                Err(reason) => (None, Some(reason)),
                            },
                            None => (None, outcome.error),
                        };
                        Outcome {
                            plugin: outcome.plugin,
                            value,
                            error,
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
            isolation: isolation.to_string(),
            signatures_required: registry.signatures_required(),
        }),
        method::SHUTDOWN => Ok(Json::Null),
        other => Err(frame::error(
            ErrorCode::MethodNotFound,
            format!("unknown method `{other}`"),
        )),
    }
}

#[cfg(feature = "signatures")]
fn signer_of(plugin: &stanchion_registry::LoadedPlugin) -> String {
    plugin.signer().to_string()
}

#[cfg(not(feature = "signatures"))]
fn signer_of(_plugin: &stanchion_registry::LoadedPlugin) -> String {
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

#[cfg(test)]
mod tests {
    /// `host/info` must report the registry's real signature posture, not a constant, so
    /// a supervising application can trust what the host attests about itself. See #46.
    #[cfg(feature = "signatures")]
    #[test]
    fn info_reports_the_real_signature_posture() {
        use super::{HostChannel, build_registry, handle};
        use crate::frame::Request;
        use crate::protocol::{HostInfo, method};
        use stanchion_lua::config::HostConfig;

        let posture = |required: bool| -> bool {
            let mut config = HostConfig::default();
            config.signatures.required = required;
            // The channel is unused by `host/info`; empty pipes suffice.
            let channel = HostChannel::new(std::io::empty(), std::io::sink());
            let (mut registry, isolation) =
                build_registry(&config, &channel).expect("build registry");
            let request = Request::new(1, method::INFO, serde_json::Value::Null);
            let value =
                handle(&mut registry, isolation, request).expect("info should succeed");
            let info: HostInfo = serde_json::from_value(value).expect("decode HostInfo");
            info.signatures_required
        };

        assert!(posture(true), "a host requiring signatures must report it");
        assert!(
            !posture(false),
            "a permissive host must not claim to require them"
        );
    }
}

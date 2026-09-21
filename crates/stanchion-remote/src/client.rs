//! The application side: launching a host process and talking to it.

use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::io::{self, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value as Json;

use jsonrpsee_types::Id;

use crate::frame::{self, Incoming, Request, Response};

use super::protocol::{
    AuditEntry, CallParams, CallbackCall, DispatchParams, HostInfo, LoadResult, Outcome,
    PluginInfo, PluginParams, RevokeParams, RootParams, error_code, method,
};

/// Something went wrong talking to the host process.
#[derive(Debug)]
pub enum RemoteError {
    /// The host binary could not be spawned.
    Spawn { program: PathBuf, source: io::Error },
    /// The pipe to or from the host failed.
    Transport(io::Error),
    /// The host exited, so no further requests can be served.
    ///
    /// This is the case the whole design exists for: a plugin that crashes the
    /// interpreter kills the host, not the application.
    ///
    /// Exactly one of the two fields carries the answer. `signal` is the interesting
    /// one: a segfault in a C rock, or an abort, kills the child rather than returning
    /// from it, so there is no exit code to report — naming the signal is the only way
    /// that crash is distinguishable from an orderly exit.
    HostGone {
        /// Exit code, when the host returned one.
        status: Option<i32>,
        /// Signal that killed the host, on platforms that have them.
        signal: Option<i32>,
    },
    /// The host answered, but with a failure.
    Host(String),
    /// The host answered with a reply that does not fit the request.
    Protocol(String),
    /// A plugin called a capability and nothing was registered to answer it.
    UnhandledCallback(String),
}

impl fmt::Display for RemoteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RemoteError::Spawn { program, source } => {
                write!(f, "could not launch `{}`: {source}", program.display())
            }
            RemoteError::Transport(source) => write!(f, "host transport failed: {source}"),
            RemoteError::HostGone { status, signal } => match (status, signal) {
                (_, Some(signal)) => match signal_name(*signal) {
                    Some(name) => write!(f, "the plugin host was killed by {name}"),
                    None => write!(f, "the plugin host was killed by signal {signal}"),
                },
                (Some(code), None) => write!(f, "the plugin host exited with status {code}"),
                (None, None) => f.write_str("the plugin host exited"),
            },
            RemoteError::Host(message) => f.write_str(message),
            RemoteError::Protocol(message) => write!(f, "unexpected reply: {message}"),
            RemoteError::UnhandledCallback(name) => {
                write!(
                    f,
                    "a plugin called `{name}`, which this application does not handle"
                )
            }
        }
    }
}

/// The signal that killed a process, where the platform has signals.
#[cfg(unix)]
fn killing_signal(status: &std::process::ExitStatus) -> Option<i32> {
    std::os::unix::process::ExitStatusExt::signal(status)
}

/// Windows has no signals: an abnormal end arrives as an exit code.
#[cfg(not(unix))]
fn killing_signal(_status: &std::process::ExitStatus) -> Option<i32> {
    None
}

/// Names the signals worth naming, which is fewer than one would like.
///
/// Signal numbers are not uniform across Unix — `SIGBUS` is 7 on Linux and 10 on
/// macOS, for instance — so only the ones POSIX fixes to the same number everywhere
/// are named here, and anything else is reported by number rather than mislabelled.
fn signal_name(signal: i32) -> Option<&'static str> {
    match signal {
        4 => Some("SIGILL"),
        6 => Some("SIGABRT"),
        8 => Some("SIGFPE"),
        9 => Some("SIGKILL"),
        11 => Some("SIGSEGV"),
        13 => Some("SIGPIPE"),
        15 => Some("SIGTERM"),
        _ => None,
    }
}

impl Error for RemoteError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            RemoteError::Spawn { source, .. } | RemoteError::Transport(source) => Some(source),
            _ => None,
        }
    }
}

/// How to launch the host process.
#[derive(Debug, Clone)]
pub struct RemoteOptions {
    program: PathBuf,
    args: Vec<OsString>,
    inherit_stderr: bool,
}

impl RemoteOptions {
    /// Launches `program` as the host.
    pub fn new(program: impl Into<PathBuf>) -> Self {
        RemoteOptions {
            program: program.into(),
            args: Vec::new(),
            inherit_stderr: true,
        }
    }

    /// Passes `--config FILE` to the host.
    pub fn config(mut self, path: impl AsRef<Path>) -> Self {
        self.args.push(OsString::from("--config"));
        self.args.push(path.as_ref().into());
        self
    }

    /// Passes `--plugins DIR` to the host.
    pub fn plugins(mut self, path: impl AsRef<Path>) -> Self {
        self.args.push(OsString::from("--plugins"));
        self.args.push(path.as_ref().into());
        self
    }

    /// Adds a raw argument.
    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Whether the host's stderr is inherited (the default) or discarded.
    ///
    /// Plugin logs and load failures go to stderr, so inheriting it is usually what
    /// you want; capture it yourself if you need to route those somewhere.
    pub fn inherit_stderr(mut self, inherit: bool) -> Self {
        self.inherit_stderr = inherit;
        self
    }
}

/// Answers the capability calls plugins make back into the application.
type CallbackFn = Box<dyn FnMut(&CallbackCall) -> Result<Json, String>>;

/// A plugin registry living in another process.
pub struct RemoteRegistry {
    child: Child,
    writer: BufWriter<ChildStdin>,
    reader: BufReader<ChildStdout>,
    next_id: u64,
    on_callback: Option<CallbackFn>,
}

impl RemoteRegistry {
    /// Spawns the host process.
    pub fn launch(options: RemoteOptions) -> Result<Self, RemoteError> {
        let mut command = Command::new(&options.program);
        command
            .args(&options.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(if options.inherit_stderr {
                Stdio::inherit()
            } else {
                Stdio::null()
            });

        let mut child = command.spawn().map_err(|source| RemoteError::Spawn {
            program: options.program.clone(),
            source,
        })?;

        // Both pipes were requested above, so their absence is a bug rather than a
        // condition the caller can act on — but it still must not panic.
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| RemoteError::Protocol("host stdin was not piped".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| RemoteError::Protocol("host stdout was not piped".to_string()))?;

        Ok(RemoteRegistry {
            child,
            writer: BufWriter::new(stdin),
            reader: BufReader::new(stdout),
            next_id: 1,
            on_callback: None,
        })
    }

    /// Answers the capabilities plugins call back into this application.
    ///
    /// Without a handler a plugin calling one gets a Lua error, so a host may offer a
    /// capability the application has not implemented without anything crashing.
    ///
    /// The handler receives the grant the host's policy approved, so it can re-check
    /// rather than trusting the host to have narrowed correctly.
    pub fn on_callback(
        mut self,
        handler: impl FnMut(&CallbackCall) -> Result<Json, String> + 'static,
    ) -> Self {
        self.on_callback = Some(Box::new(handler));
        self
    }

    /// Loads every plugin under `root`, returning what loaded and what did not.
    pub fn load(&mut self, root: impl AsRef<Path>) -> Result<LoadResult, RemoteError> {
        let root = root.as_ref().display().to_string();
        self.call_host(method::LOAD, RootParams { root })
    }

    /// Lists the plugins the host currently holds.
    pub fn list(&mut self) -> Result<Vec<PluginInfo>, RemoteError> {
        self.call_host(method::LIST, Json::Null)
    }

    /// Reports what plugins under `root` request, without running their code.
    pub fn audit(&mut self, root: impl AsRef<Path>) -> Result<Vec<AuditEntry>, RemoteError> {
        let root = root.as_ref().display().to_string();
        self.call_host(method::AUDIT, RootParams { root })
    }

    /// Calls one method on one plugin, deserializing the result.
    pub fn call<T: DeserializeOwned>(
        &mut self,
        plugin: &str,
        method_name: &str,
        args: impl IntoIterator<Item = Json>,
    ) -> Result<T, RemoteError> {
        self.call_host(
            method::CALL,
            CallParams {
                plugin: plugin.to_string(),
                method: method_name.to_string(),
                args: args.into_iter().collect(),
            },
        )
    }

    /// Calls the same method on every plugin, collecting one outcome each.
    ///
    /// A plugin that fails is reported in place, exactly as in-process dispatch does.
    pub fn dispatch(
        &mut self,
        method_name: &str,
        args: impl IntoIterator<Item = Json>,
    ) -> Result<Vec<Outcome>, RemoteError> {
        self.call_host(
            method::DISPATCH,
            DispatchParams {
                method: method_name.to_string(),
                args: args.into_iter().collect(),
            },
        )
    }

    /// Re-reads one plugin from disk in the host.
    pub fn reload(&mut self, plugin: &str) -> Result<(), RemoteError> {
        self.call_host(
            method::RELOAD,
            PluginParams {
                plugin: plugin.to_string(),
            },
        )
    }

    /// Unbinds a capability from a live plugin in the host.
    pub fn revoke(&mut self, plugin: &str, capability: &str) -> Result<(), RemoteError> {
        self.call_host(
            method::REVOKE,
            RevokeParams {
                plugin: plugin.to_string(),
                capability: capability.to_string(),
            },
        )
    }

    /// Asks the host to describe itself.
    pub fn info(&mut self) -> Result<HostInfo, RemoteError> {
        self.call_host(method::INFO, Json::Null)
    }

    /// Asks the host to exit, then waits for it.
    pub fn shutdown(mut self) -> Result<(), RemoteError> {
        // A host that already died is not an error to shut down.
        let _: Result<Json, _> = self.call_host(method::SHUTDOWN, Json::Null);
        self.child.wait().map_err(RemoteError::Transport)?;
        Ok(())
    }

    /// Whether the host process is still running.
    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Sends one request and reads its reply, answering any callback that arrives
    /// while waiting, and mapping a dead host to `HostGone`.
    fn call_host<P: Serialize, T: DeserializeOwned>(
        &mut self,
        method_name: &str,
        params: P,
    ) -> Result<T, RemoteError> {
        let number = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let params =
            serde_json::to_value(params).map_err(|err| RemoteError::Protocol(err.to_string()))?;

        if let Err(err) = frame::write(&mut self.writer, &Request::new(number, method_name, params))
        {
            return Err(self.diagnose(err));
        }
        let id = Id::Number(number);

        loop {
            match frame::read(&mut self.reader) {
                Err(err) => return Err(self.diagnose(err)),
                // A closed stream means the child is gone, which is the case the
                // whole design exists for.
                Ok(None) => return Err(self.gone()),
                Ok(Some(Incoming::Response(response))) if response.id == id => {
                    if let Some(error) = response.error {
                        return Err(RemoteError::Host(error.message().to_string()));
                    }
                    let value = response.result.unwrap_or(Json::Null);
                    return serde_json::from_value(value)
                        .map_err(|err| RemoteError::Protocol(err.to_string()));
                }
                Ok(Some(Incoming::Response(_))) => continue,
                Ok(Some(Incoming::Request(callback))) => self.answer_callback(callback)?,
            }
        }
    }

    /// Answers a capability call a plugin made back into this application.
    fn answer_callback(&mut self, callback: Request) -> Result<(), RemoteError> {
        let name = callback
            .method
            .strip_prefix(method::CAPABILITY_PREFIX)
            .unwrap_or(&callback.method)
            .to_string();

        let outcome =
            match serde_json::from_value::<CallbackCall>(callback.params.unwrap_or(Json::Null)) {
                Err(err) => Err(format!("malformed callback: {err}")),
                Ok(call) => match self.on_callback.as_mut() {
                    Some(handler) => handler(&call),
                    None => Err(format!(
                        "this application does not handle the `{name}` capability"
                    )),
                },
            };

        let response = match outcome {
            Ok(value) => Response::ok(callback.id, value),
            Err(message) => Response::failed(
                callback.id,
                frame::app_error(error_code::REQUEST_FAILED, message),
            ),
        };

        frame::write(&mut self.writer, &response).map_err(|err| self.diagnose(err))
    }

    /// Distinguishes a dead host from an ordinary transport failure.
    fn diagnose(&mut self, err: io::Error) -> RemoteError {
        match self.child.try_wait() {
            Ok(Some(_)) => self.gone(),
            _ => RemoteError::Transport(err),
        }
    }

    fn gone(&mut self) -> RemoteError {
        let Ok(status) = self.child.wait() else {
            return RemoteError::HostGone {
                status: None,
                signal: None,
            };
        };
        RemoteError::HostGone {
            status: status.code(),
            signal: killing_signal(&status),
        }
    }
}

impl Drop for RemoteRegistry {
    fn drop(&mut self) {
        // Closing stdin ends the host's read loop; kill anything that ignores it so a
        // dropped registry never leaks a process.
        let _ = self.writer.flush();
        if let Some(stdin) = self.child.stdin.take() {
            drop(stdin);
        }
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

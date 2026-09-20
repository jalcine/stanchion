//! The application side: launching a host process and talking to it.

use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::io::{self, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde::de::DeserializeOwned;
use serde_json::Value as Json;

use super::protocol::{
    AuditEntry, Envelope, Failure, Outcome, PluginInfo, Request, Response, read_message,
    write_message,
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
    HostGone { status: Option<i32> },
    /// The host answered, but with a failure.
    Host(String),
    /// The host answered with a reply that does not fit the request.
    Protocol(String),
}

impl fmt::Display for RemoteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RemoteError::Spawn { program, source } => {
                write!(f, "could not launch `{}`: {source}", program.display())
            }
            RemoteError::Transport(source) => write!(f, "host transport failed: {source}"),
            RemoteError::HostGone { status } => match status {
                Some(code) => write!(f, "the plugin host exited with status {code}"),
                None => f.write_str("the plugin host exited"),
            },
            RemoteError::Host(message) => f.write_str(message),
            RemoteError::Protocol(message) => write!(f, "unexpected reply: {message}"),
        }
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

/// A plugin registry living in another process.
pub struct RemoteRegistry {
    child: Child,
    writer: BufWriter<ChildStdin>,
    reader: BufReader<ChildStdout>,
    next_id: u64,
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
        })
    }

    /// Loads every plugin under `root`, returning what loaded and what did not.
    pub fn load(&mut self, root: impl AsRef<Path>) -> Result<LoadOutcome, RemoteError> {
        let root = root.as_ref().display().to_string();
        match self.request(Request::Load { root })? {
            Response::Loaded { loaded, failures } => Ok(LoadOutcome { loaded, failures }),
            other => Err(unexpected(&other)),
        }
    }

    /// Lists the plugins the host currently holds.
    pub fn list(&mut self) -> Result<Vec<PluginInfo>, RemoteError> {
        match self.request(Request::List)? {
            Response::Plugins { plugins } => Ok(plugins),
            other => Err(unexpected(&other)),
        }
    }

    /// Reports what plugins under `root` request, without running their code.
    pub fn audit(&mut self, root: impl AsRef<Path>) -> Result<Vec<AuditEntry>, RemoteError> {
        let root = root.as_ref().display().to_string();
        match self.request(Request::Audit { root })? {
            Response::Audit { entries } => Ok(entries),
            other => Err(unexpected(&other)),
        }
    }

    /// Calls one method on one plugin, deserializing the result.
    pub fn call<T: DeserializeOwned>(
        &mut self,
        plugin: &str,
        method: &str,
        args: impl IntoIterator<Item = Json>,
    ) -> Result<T, RemoteError> {
        let request = Request::Call {
            plugin: plugin.to_string(),
            method: method.to_string(),
            args: args.into_iter().collect(),
        };
        match self.request(request)? {
            Response::Value { value } => serde_json::from_value(value)
                .map_err(|err| RemoteError::Protocol(err.to_string())),
            other => Err(unexpected(&other)),
        }
    }

    /// Calls the same method on every plugin, collecting one outcome each.
    ///
    /// A plugin that fails is reported in place, exactly as in-process dispatch does.
    pub fn dispatch(
        &mut self,
        method: &str,
        args: impl IntoIterator<Item = Json>,
    ) -> Result<Vec<Outcome>, RemoteError> {
        let request = Request::Dispatch {
            method: method.to_string(),
            args: args.into_iter().collect(),
        };
        match self.request(request)? {
            Response::Outcomes { outcomes } => Ok(outcomes),
            other => Err(unexpected(&other)),
        }
    }

    /// Re-reads one plugin from disk in the host.
    pub fn reload(&mut self, plugin: &str) -> Result<(), RemoteError> {
        self.expect_ok(Request::Reload { plugin: plugin.to_string() })
    }

    /// Unbinds a capability from a live plugin in the host.
    pub fn revoke(&mut self, plugin: &str, capability: &str) -> Result<(), RemoteError> {
        self.expect_ok(Request::Revoke {
            plugin: plugin.to_string(),
            capability: capability.to_string(),
        })
    }

    /// Asks the host to describe itself.
    pub fn info(&mut self) -> Result<Response, RemoteError> {
        self.request(Request::Info)
    }

    /// Asks the host to exit, then waits for it.
    pub fn shutdown(mut self) -> Result<(), RemoteError> {
        // A host that already died is not an error to shut down.
        let _ = self.expect_ok(Request::Shutdown);
        self.child.wait().map_err(RemoteError::Transport)?;
        Ok(())
    }

    /// Whether the host process is still running.
    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn expect_ok(&mut self, request: Request) -> Result<(), RemoteError> {
        match self.request(request)? {
            Response::Ok => Ok(()),
            other => Err(unexpected(&other)),
        }
    }

    /// Sends one request and reads its reply, mapping a dead host to `HostGone`.
    fn request(&mut self, request: Request) -> Result<Response, RemoteError> {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);

        if let Err(err) = write_message(&mut self.writer, &Envelope { id, body: request }) {
            return Err(self.diagnose(err));
        }

        match read_message::<_, Envelope<Response>>(&mut self.reader) {
            Ok(Some(envelope)) if envelope.id == id => match envelope.body {
                Response::Error { message } => Err(RemoteError::Host(message)),
                body => Ok(body),
            },
            Ok(Some(envelope)) => Err(RemoteError::Protocol(format!(
                "reply {} does not match request {id}",
                envelope.id
            ))),
            // A closed stream means the child is gone, which is the interesting case.
            Ok(None) => Err(self.gone()),
            Err(err) => Err(self.diagnose(err)),
        }
    }

    /// Distinguishes a dead host from an ordinary transport failure.
    fn diagnose(&mut self, err: io::Error) -> RemoteError {
        match self.child.try_wait() {
            Ok(Some(_)) => self.gone(),
            _ => RemoteError::Transport(err),
        }
    }

    fn gone(&mut self) -> RemoteError {
        let status = self.child.wait().ok().and_then(|status| status.code());
        RemoteError::HostGone { status }
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

/// What one remote load did.
#[derive(Debug, Clone)]
pub struct LoadOutcome {
    /// Plugins that loaded, in order.
    pub loaded: Vec<String>,
    /// Plugins that did not, with the host's reason.
    pub failures: Vec<Failure>,
}

impl LoadOutcome {
    /// True when every discovered plugin loaded.
    pub fn is_clean(&self) -> bool {
        self.failures.is_empty()
    }
}

fn unexpected(response: &Response) -> RemoteError {
    RemoteError::Protocol(format!("{response:?}"))
}

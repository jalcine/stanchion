//! Kotlin and Swift bindings for stanchion.
//!
//! Python came first, and a seam shaped by its first consumer is a seam the other
//! four have to work around. UniFFI is the strictest of the five — it needs owned,
//! `Send + Sync` types, declared error types and foreign traits named up front — so
//! this crate is what keeps `stanchion-ffi` honest. If a `pyo3` convenience ever
//! leaks into the shared shape, this stops building.
//!
//! Compiling is not the bar, though, and three things here were found only by
//! running: a flat error cannot be lifted back out of foreign code, a variant named
//! `List` shadows `kotlin.collections.List` inside its own sealed class, and neither
//! shows up until the generated bindings are compiled and executed. `smoke.sh` and
//! `smoke-kotlin.sh` do that, covering values, capability providers, policy
//! narrowing, the reentrancy guard and `suspend`/`async` calls.
//!
//! What remains: packaging. The `XCFramework` and Swift Package layout, the AAR with
//! its per-ABI libraries, and a Swift run of the smoke tests — the Swift bindings
//! generate but have not been executed, since this is a Linux checkout with no Swift
//! toolchain.

use std::sync::Arc;

use stanchion_ffi::{
    AuditEntry as FfiAudit, CapabilityCall as FfiCall, CapabilityProvider as FfiProvider,
    CapabilityRequest as FfiRequest, Decision as FfiDecision, Error as FfiError, HostConfig,
    LoadReport as FfiReport, Outcome as FfiOutcome, PluginInfo as FfiInfo,
    Policy as FfiPolicy, Stanchion as Host, Value as FfiValue,
};

uniffi::setup_scaffolding!();

/// A value crossing between a plugin and a foreign host.
///
/// UniFFI renders a recursive enum as a sealed class in Kotlin and an indirect enum
/// in Swift, which is exactly the shape `stanchion_ffi::Value` already has.
///
/// The sequence and mapping variants are `Seq` and `Table` rather than the `List` and
/// `Map` they mirror. A variant named `List` becomes a nested class inside the sealed
/// class in Kotlin, which shadows `kotlin.collections.List` for every member
/// declared after it — so its own `items: List<Value>` field resolves to the variant
/// rather than to a list, and the generated file does not compile. `Table` is also
/// what Lua calls both of these anyway.
#[derive(Clone, Debug, PartialEq, uniffi::Enum)]
pub enum Value {
    Nil,
    Bool { value: bool },
    Int { value: i64 },
    Float { value: f64 },
    Str { value: String },
    Seq { items: Vec<Value> },
    Table { entries: std::collections::HashMap<String, Value> },
    Function,
}

impl From<&FfiValue> for Value {
    fn from(value: &FfiValue) -> Self {
        match value {
            FfiValue::Nil => Value::Nil,
            FfiValue::Bool(value) => Value::Bool { value: *value },
            FfiValue::Int(value) => Value::Int { value: *value },
            FfiValue::Float(value) => Value::Float { value: *value },
            FfiValue::Str(value) => Value::Str {
                value: value.clone(),
            },
            FfiValue::List(items) => Value::Seq {
                items: items.iter().map(Value::from).collect(),
            },
            FfiValue::Map(entries) => Value::Table {
                entries: entries
                    .iter()
                    .map(|(key, entry)| (key.clone(), Value::from(entry)))
                    .collect(),
            },
            FfiValue::Function => Value::Function,
        }
    }
}

impl From<&Value> for FfiValue {
    fn from(value: &Value) -> Self {
        match value {
            Value::Nil => FfiValue::Nil,
            Value::Bool { value } => FfiValue::Bool(*value),
            Value::Int { value } => FfiValue::Int(*value),
            Value::Float { value } => FfiValue::Float(*value),
            Value::Str { value } => FfiValue::Str(value.clone()),
            Value::Seq { items } => FfiValue::List(items.iter().map(FfiValue::from).collect()),
            Value::Table { entries } => FfiValue::Map(
                entries
                    .iter()
                    .map(|(key, entry)| (key.clone(), FfiValue::from(entry)))
                    .collect(),
            ),
            Value::Function => FfiValue::Function,
        }
    }
}

/// What went wrong.
#[derive(Debug, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum StanchionError {
    #[error("{0}")]
    UnknownPlugin(String),
    #[error("{0}")]
    Plugin(String),
    #[error("{0}")]
    Lua(String),
    #[error("{0}")]
    Io(String),
    #[error("{0}")]
    Config(String),
    #[error("{0}")]
    Capability(String),
    #[error("{0}")]
    Reentrant(String),
}

impl From<FfiError> for StanchionError {
    fn from(err: FfiError) -> Self {
        let message = err.to_string();
        match err.kind() {
            "unknown-plugin" => StanchionError::UnknownPlugin(message),
            "plugin" => StanchionError::Plugin(message),
            "lua" => StanchionError::Lua(message),
            "io" => StanchionError::Io(message),
            "capability" => StanchionError::Capability(message),
            "reentrant" => StanchionError::Reentrant(message),
            _ => StanchionError::Config(message),
        }
    }
}

type Result<T> = std::result::Result<T, StanchionError>;

/// One plugin that did not load.
#[derive(Clone, Debug, uniffi::Record)]
pub struct Failure {
    pub plugin: String,
    pub reason: String,
}

/// What one `load` call did.
#[derive(Clone, Debug, uniffi::Record)]
pub struct LoadReport {
    pub loaded: Vec<String>,
    pub failures: Vec<Failure>,
}

/// A loaded plugin.
#[derive(Clone, Debug, uniffi::Record)]
pub struct PluginInfo {
    pub name: String,
    pub version: Option<String>,
    pub granted: Vec<String>,
    pub signer: String,
}

/// What one plugin requests, read without running it.
#[derive(Clone, Debug, uniffi::Record)]
pub struct AuditEntry {
    pub plugin: String,
    pub capabilities: Vec<String>,
    pub signer: String,
}

/// One plugin's result from a dispatch.
#[derive(Clone, Debug, uniffi::Record)]
pub struct Outcome {
    pub plugin: String,
    pub value: Option<Value>,
    pub error: Option<String>,
}

/// A plugin reaching back into foreign code through a granted capability.
#[derive(Clone, Debug, uniffi::Record)]
pub struct CapabilityCall {
    pub plugin: String,
    pub capability: String,
    /// The parameters policy approved, which may be narrower than requested.
    pub grant: Value,
    pub args: Vec<Value>,
}

/// What a plugin asked for, before any policy ran.
#[derive(Clone, Debug, uniffi::Record)]
pub struct CapabilityRequest {
    pub plugin: String,
    pub capability: String,
    pub params: Value,
    pub optional: bool,
    pub signer: String,
}

/// A policy's answer to one request.
#[derive(Clone, Debug, uniffi::Enum)]
pub enum Decision {
    Grant,
    GrantWith { params: Value },
    Deny { reason: String },
}

/// Why a provider could not answer.
///
/// A bare `String` would do on the Rust side, but a foreign trait has to declare a
/// real error type: UniFFI turns this into an exception the foreign implementation
/// throws, and there is no exception class for "some string".
///
/// Deliberately *not* `flat_error`, unlike [`StanchionError`]. A flat error can be
/// lowered into a foreign exception but never lifted back out, and this one travels
/// the other way — foreign code raises it and Rust has to read it. Marking it flat
/// compiles and then fails at the boundary with `Can't lift flat errors`, turning
/// every refusal into an internal error instead of the catchable Lua error a plugin
/// is supposed to see.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum ProviderError {
    /// The provider refused, or failed while doing the work.
    #[error("{reason}")]
    Refused { reason: String },
}

/// Answers a capability a plugin calls.
///
/// Throwing surfaces inside Lua as an ordinary runtime error the plugin can `pcall`,
/// so refusing is a normal outcome rather than a fatal one.
///
/// Do not call back into the `Stanchion` that invoked this: it holds the registry's
/// lock, so the attempt fails with `Reentrant` rather than deadlocking.
#[uniffi::export(with_foreign)]
pub trait CapabilityProvider: Send + Sync {
    fn invoke(&self, call: CapabilityCall) -> std::result::Result<Value, ProviderError>;
}

/// Decides what each plugin may actually have.
#[uniffi::export(with_foreign)]
pub trait Policy: Send + Sync {
    fn decide(&self, request: CapabilityRequest) -> Decision;
}

/// Adapts a foreign provider onto the seam's trait.
struct ProviderBridge {
    inner: Arc<dyn CapabilityProvider>,
}

impl FfiProvider for ProviderBridge {
    fn invoke(&self, call: &FfiCall) -> std::result::Result<FfiValue, String> {
        let call = CapabilityCall {
            plugin: call.plugin.clone(),
            capability: call.capability.clone(),
            grant: Value::from(&call.grant),
            args: call.args.iter().map(Value::from).collect(),
        };
        self.inner
            .invoke(call)
            .map(|value| FfiValue::from(&value))
            .map_err(|err| err.to_string())
    }
}

/// Adapts a foreign policy onto the seam's trait.
struct PolicyBridge {
    inner: Arc<dyn Policy>,
}

impl FfiPolicy for PolicyBridge {
    fn decide(&self, request: &FfiRequest) -> FfiDecision {
        let request = CapabilityRequest {
            plugin: request.plugin.clone(),
            capability: request.capability.clone(),
            params: Value::from(&request.params),
            optional: request.optional,
            signer: request.signer.clone(),
        };
        match self.inner.decide(request) {
            Decision::Grant => FfiDecision::Grant,
            Decision::GrantWith { params } => FfiDecision::GrantWith(FfiValue::from(&params)),
            Decision::Deny { reason } => FfiDecision::Deny(reason),
        }
    }
}

/// How a host runs plugins.
#[derive(Clone, Debug, Default, uniffi::Record)]
pub struct Config {
    pub plugins: Option<String>,
    pub libs: Option<Vec<String>>,
    pub deny: Option<Vec<String>>,
    pub memory_limit: Option<u64>,
    pub instruction_limit: Option<u64>,
    #[uniffi(default = false)]
    pub shared: bool,
    #[uniffi(default = false)]
    pub require_signatures: bool,
    #[uniffi(default = [])]
    pub allow: Vec<String>,
}

/// A registry of Lua plugins.
#[derive(uniffi::Object)]
pub struct Stanchion {
    inner: Host,
}

#[uniffi::export(async_runtime = "tokio")]
impl Stanchion {
    /// Builds a registry.
    #[uniffi::constructor]
    pub fn new(
        config: Config,
        capabilities: std::collections::HashMap<String, Arc<dyn CapabilityProvider>>,
        policy: Option<Arc<dyn Policy>>,
    ) -> Result<Stanchion> {
        let mut host = HostConfig {
            plugins: config.plugins.map(Into::into),
            ..HostConfig::default()
        };
        host.sandbox.shared = config.shared;
        host.sandbox.libs = config.libs;
        host.sandbox.deny = config.deny;
        host.signatures.required = config.require_signatures;
        host.capabilities.allow = config.allow;
        if let Some(bytes) = config.memory_limit {
            host.sandbox.memory_limit = (bytes > 0).then_some(bytes as usize);
        }
        if let Some(instructions) = config.instruction_limit {
            host.sandbox.instruction_limit = (instructions > 0).then_some(instructions);
        }

        let mut builder = Host::builder().config(host);
        for (name, provider) in capabilities {
            builder = builder.capability(
                name,
                Arc::new(ProviderBridge { inner: provider }) as Arc<dyn FfiProvider>,
            );
        }
        if let Some(policy) = policy {
            builder =
                builder.policy(Arc::new(PolicyBridge { inner: policy }) as Arc<dyn FfiPolicy>);
        }
        Ok(Stanchion {
            inner: builder.build()?,
        })
    }

    /// Discovers and loads every plugin under a root.
    pub fn load(&self, root: Option<String>) -> Result<LoadReport> {
        let report: FfiReport = self.inner.load(root.as_ref().map(std::path::Path::new))?;
        Ok(LoadReport {
            loaded: report.loaded,
            failures: report
                .failures
                .into_iter()
                .map(|failure| Failure {
                    plugin: failure.plugin,
                    reason: failure.reason,
                })
                .collect(),
        })
    }

    /// Reports what plugins request, without running any of their code.
    pub fn audit(&self, root: Option<String>) -> Result<Vec<AuditEntry>> {
        let entries: Vec<FfiAudit> = self.inner.audit(root.as_ref().map(std::path::Path::new))?;
        Ok(entries
            .into_iter()
            .map(|entry| AuditEntry {
                plugin: entry.plugin,
                capabilities: entry.capabilities,
                signer: entry.signer,
            })
            .collect())
    }

    /// The loaded plugins.
    pub fn list(&self) -> Result<Vec<PluginInfo>> {
        let plugins: Vec<FfiInfo> = self.inner.list()?;
        Ok(plugins
            .into_iter()
            .map(|plugin| PluginInfo {
                name: plugin.name,
                version: plugin.version,
                granted: plugin.granted,
                signer: plugin.signer,
            })
            .collect())
    }

    /// The loaded plugins' names.
    pub fn names(&self) -> Result<Vec<String>> {
        Ok(self.inner.names()?)
    }

    /// Calls one method on one plugin.
    pub fn call(
        &self,
        plugin: String,
        method: String,
        args: Vec<Value>,
    ) -> Result<Value> {
        let args: Vec<FfiValue> = args.iter().map(FfiValue::from).collect();
        Ok(Value::from(&self.inner.call(&plugin, &method, &args)?))
    }

    /// Calls the same method on every plugin, collecting one result each.
    pub fn dispatch(
        &self,
        method: String,
        args: Vec<Value>,
    ) -> Result<Vec<Outcome>> {
        let args: Vec<FfiValue> = args.iter().map(FfiValue::from).collect();
        Ok(outcomes(self.inner.dispatch(&method, &args)?))
    }

    /// Awaits one method on one plugin, which may yield.
    ///
    /// Kotlin sees a `suspend fun`; Swift sees `async`.
    pub async fn call_async(
        &self,
        plugin: String,
        method: String,
        args: Vec<Value>,
    ) -> Result<Value> {
        let args: Vec<FfiValue> = args.iter().map(FfiValue::from).collect();
        let value = self.inner.call_async(&plugin, &method, &args).await?;
        Ok(Value::from(&value))
    }

    /// Awaits the same method on every plugin, in turn.
    pub async fn dispatch_async(
        &self,
        method: String,
        args: Vec<Value>,
    ) -> Result<Vec<Outcome>> {
        let args: Vec<FfiValue> = args.iter().map(FfiValue::from).collect();
        Ok(outcomes(self.inner.dispatch_async(&method, &args).await?))
    }

    /// Re-reads one plugin from disk.
    pub fn reload(&self, plugin: String) -> Result<()> {
        Ok(self.inner.reload(&plugin)?)
    }

    /// Unbinds a granted capability from a live plugin.
    pub fn revoke(&self, plugin: String, capability: String) -> Result<bool> {
        Ok(self.inner.revoke(&plugin, &capability)?)
    }

    /// Whether plugins share one Lua state: `"shared"` or `"per-plugin"`.
    pub fn isolation(&self) -> Result<String> {
        Ok(self.inner.isolation()?.to_string())
    }

    /// How many plugins are loaded.
    pub fn count(&self) -> Result<u64> {
        Ok(self.inner.len()? as u64)
    }
}

fn outcomes(outcomes: Vec<FfiOutcome>) -> Vec<Outcome> {
    outcomes
        .into_iter()
        .map(|outcome| Outcome {
            plugin: outcome.plugin,
            value: outcome.value.as_ref().map(Value::from),
            error: outcome.error,
        })
        .collect()
}

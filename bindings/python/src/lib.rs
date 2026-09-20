//! Python bindings for stanchion.
//!
//! A thin translation layer over `stanchion-ffi`: Python objects in, `Value`s out,
//! Python callables wrapped as capability providers and policies. Everything that
//! decides *behaviour* lives in `stanchion-ffi`, so this binding and the other four
//! cannot drift apart.

use std::sync::Arc;

use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyTuple};

use stanchion_ffi::{
    CapabilityCall as FfiCall, CapabilityProvider, CapabilityRequest as FfiRequest, Decision as FfiDecision,
    Error as FfiError, HostConfig, Policy as FfiPolicy, Stanchion as Host, Value,
};

mod value;

use value::{from_py, to_py};

create_exception!(_stanchion, StanchionError, PyException, "Base for every stanchion failure.");
create_exception!(_stanchion, UnknownPluginError, StanchionError, "No plugin by that name is loaded.");
create_exception!(_stanchion, PluginError, StanchionError, "A plugin failed to load, reload or verify.");
create_exception!(_stanchion, LuaError, StanchionError, "A plugin's Lua raised.");
create_exception!(_stanchion, ConfigError, StanchionError, "The host's own configuration is wrong.");
create_exception!(_stanchion, CapabilityError, StanchionError, "A capability provider refused or failed.");
create_exception!(
    _stanchion,
    ReentrantError,
    StanchionError,
    "A capability provider called back into the registry that invoked it."
);

/// Maps a failure onto the exception class that names it.
fn raise(err: FfiError) -> PyErr {
    let message = err.to_string();
    match err.kind() {
        "unknown-plugin" => UnknownPluginError::new_err(message),
        "plugin" => PluginError::new_err(message),
        "lua" => LuaError::new_err(message),
        "config" => ConfigError::new_err(message),
        "capability" => CapabilityError::new_err(message),
        "reentrant" => ReentrantError::new_err(message),
        _ => StanchionError::new_err(message),
    }
}

// ---- what a provider and a policy are handed -------------------------------

/// A plugin reaching back into Python through a granted capability.
#[pyclass(module = "stanchion", frozen, get_all, skip_from_py_object)]
pub struct CapabilityCall {
    /// Plugin making the call.
    pub plugin: String,
    /// Capability it was granted.
    pub capability: String,
    /// The parameters policy approved, which may be narrower than the manifest asked
    /// for. Re-check against these rather than trusting the grant to have narrowed.
    pub grant: Py<PyAny>,
    /// Arguments the plugin passed.
    pub args: Vec<Py<PyAny>>,
}

#[pymethods]
impl CapabilityCall {
    fn __repr__(&self) -> String {
        format!(
            "CapabilityCall(plugin={:?}, capability={:?}, args={})",
            self.plugin,
            self.capability,
            self.args.len()
        )
    }
}

/// What a plugin asked for, before any policy ran.
#[pyclass(module = "stanchion", frozen, get_all, skip_from_py_object)]
pub struct CapabilityRequest {
    pub plugin: String,
    pub capability: String,
    pub params: Py<PyAny>,
    pub optional: bool,
    /// Who signed the plugin, or `"unsigned"`.
    pub signer: String,
}

#[pymethods]
impl CapabilityRequest {
    fn __repr__(&self) -> String {
        format!(
            "CapabilityRequest(plugin={:?}, capability={:?}, signer={:?})",
            self.plugin, self.capability, self.signer
        )
    }
}

/// A policy's answer to one request.
///
/// Built through the three constructors rather than by returning bare values, so that
/// "grant nothing" and "grant with no parameters" cannot be confused.
#[pyclass(module = "stanchion", frozen, from_py_object)]
#[derive(Clone)]
pub struct Decision {
    inner: FfiDecision,
}

#[pymethods]
impl Decision {
    /// Grant exactly what was asked for.
    #[staticmethod]
    fn grant() -> Decision {
        Decision {
            inner: FfiDecision::Grant,
        }
    }

    /// Grant, but with these parameters instead of the requested ones.
    #[staticmethod]
    fn grant_with(params: &Bound<'_, PyAny>) -> PyResult<Decision> {
        Ok(Decision {
            inner: FfiDecision::GrantWith(from_py(params)?),
        })
    }

    /// Refuse, with a reason the plugin author will see.
    #[staticmethod]
    fn deny(reason: String) -> Decision {
        Decision {
            inner: FfiDecision::Deny(reason),
        }
    }

    fn __repr__(&self) -> String {
        match &self.inner {
            FfiDecision::Grant => "Decision.grant()".to_string(),
            FfiDecision::GrantWith(_) => "Decision.grant_with(...)".to_string(),
            FfiDecision::Deny(reason) => format!("Decision.deny({reason:?})"),
        }
    }
}

// ---- bridging Python callables ---------------------------------------------

/// A Python callable answering a capability.
struct PyProvider {
    callable: Py<PyAny>,
}

impl CapabilityProvider for PyProvider {
    fn invoke(&self, call: &FfiCall) -> Result<Value, String> {
        // Re-acquires the GIL the calling thread released before entering Lua. Same
        // thread, so this cannot deadlock against that release.
        Python::attach(|py| {
            let call = CapabilityCall {
                plugin: call.plugin.clone(),
                capability: call.capability.clone(),
                grant: to_py(py, &call.grant)
                    .map_err(|err| err.to_string())?
                    .unbind(),
                args: call
                    .args
                    .iter()
                    .map(|arg| to_py(py, arg).map(Bound::unbind))
                    .collect::<PyResult<Vec<_>>>()
                    .map_err(|err| err.to_string())?,
            };
            let answer = self
                .callable
                .call1(py, (call,))
                .map_err(|err| err.to_string())?;
            from_py(answer.bind(py)).map_err(|err| err.to_string())
        })
    }
}

/// A Python callable deciding policy.
struct PyPolicy {
    callable: Py<PyAny>,
}

impl FfiPolicy for PyPolicy {
    fn decide(&self, request: &FfiRequest) -> FfiDecision {
        Python::attach(|py| {
            let built = to_py(py, &request.params)
                .map(Bound::unbind)
                .map(|params| CapabilityRequest {
                    plugin: request.plugin.clone(),
                    capability: request.capability.clone(),
                    params,
                    optional: request.optional,
                    signer: request.signer.clone(),
                });

            // A policy that raises denies. Letting the exception escape would unwind
            // through Lua and the registry, and a host whose policy is broken should
            // refuse rather than crash.
            let decided = built.and_then(|request| self.callable.call1(py, (request,)));
            match decided {
                Err(err) => FfiDecision::Deny(format!("the policy raised: {err}")),
                Ok(answer) => match answer.extract::<Decision>(py) {
                    Ok(decision) => decision.inner,
                    Err(_) => FfiDecision::Deny(
                        "a policy must return a Decision: grant(), grant_with(...) or deny(...)"
                            .to_string(),
                    ),
                },
            }
        })
    }
}

// ---- reports ----------------------------------------------------------------

/// One plugin that did not load.
#[pyclass(module = "stanchion", frozen, get_all, skip_from_py_object)]
#[derive(Clone)]
pub struct Failure {
    pub plugin: String,
    pub reason: String,
}

#[pymethods]
impl Failure {
    fn __repr__(&self) -> String {
        format!("Failure(plugin={:?}, reason={:?})", self.plugin, self.reason)
    }
}

/// What one `load` call did.
#[pyclass(module = "stanchion", frozen, get_all, skip_from_py_object)]
#[derive(Clone)]
pub struct LoadReport {
    /// Names of plugins that loaded, in the order they were constructed.
    pub loaded: Vec<String>,
    /// Plugins that did not. One failing never stops the others.
    pub failures: Vec<Failure>,
}

#[pymethods]
impl LoadReport {
    /// True when every discovered plugin loaded.
    #[getter]
    fn is_clean(&self) -> bool {
        self.failures.is_empty()
    }

    fn __repr__(&self) -> String {
        format!(
            "LoadReport(loaded={:?}, failures={})",
            self.loaded,
            self.failures.len()
        )
    }
}

/// A loaded plugin.
#[pyclass(module = "stanchion", frozen, get_all, skip_from_py_object)]
#[derive(Clone)]
pub struct PluginInfo {
    pub name: String,
    pub version: Option<String>,
    pub granted: Vec<String>,
    pub signer: String,
}

#[pymethods]
impl PluginInfo {
    fn __repr__(&self) -> String {
        format!(
            "PluginInfo(name={:?}, version={:?}, signer={:?})",
            self.name, self.version, self.signer
        )
    }
}

/// What one plugin requests, read without running it.
#[pyclass(module = "stanchion", frozen, get_all, skip_from_py_object)]
#[derive(Clone)]
pub struct AuditEntry {
    pub plugin: String,
    pub capabilities: Vec<String>,
    pub signer: String,
}

#[pymethods]
impl AuditEntry {
    fn __repr__(&self) -> String {
        format!(
            "AuditEntry(plugin={:?}, capabilities={:?})",
            self.plugin, self.capabilities
        )
    }
}

/// One plugin's result from a dispatch.
#[pyclass(module = "stanchion", frozen, get_all, skip_from_py_object)]
pub struct Outcome {
    pub plugin: String,
    /// Present when the call succeeded.
    pub value: Option<Py<PyAny>>,
    /// Present when it failed. One plugin failing never affects the others.
    pub error: Option<String>,
}

#[pymethods]
impl Outcome {
    fn __repr__(&self) -> String {
        match &self.error {
            Some(error) => format!("Outcome(plugin={:?}, error={:?})", self.plugin, error),
            None => format!("Outcome(plugin={:?}, ok)", self.plugin),
        }
    }
}

fn outcomes_to_py(py: Python<'_>, outcomes: Vec<stanchion_ffi::Outcome>) -> PyResult<Vec<Outcome>> {
    outcomes
        .into_iter()
        .map(|outcome| {
            Ok(Outcome {
                plugin: outcome.plugin,
                value: outcome
                    .value
                    .map(|value| to_py(py, &value).map(Bound::unbind))
                    .transpose()?,
                error: outcome.error,
            })
        })
        .collect()
}

// ---- the registry -----------------------------------------------------------

/// A registry of Lua plugins.
#[pyclass(module = "stanchion", frozen)]
pub struct Stanchion {
    inner: Arc<Host>,
}

#[pymethods]
impl Stanchion {
    /// Builds a registry.
    ///
    /// Capabilities and policy are fixed here and cannot change afterwards: a host
    /// that could widen a running plugin's reach would have given up the guarantee
    /// the capability system exists to make.
    #[new]
    #[pyo3(signature = (
        *,
        plugins = None,
        capabilities = None,
        policy = None,
        allow = None,
        libs = None,
        deny = None,
        memory_limit = None,
        instruction_limit = None,
        shared = false,
        require_signatures = false,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        plugins: Option<std::path::PathBuf>,
        capabilities: Option<&Bound<'_, PyDict>>,
        policy: Option<Py<PyAny>>,
        allow: Option<Vec<String>>,
        libs: Option<Vec<String>>,
        deny: Option<Vec<String>>,
        memory_limit: Option<usize>,
        instruction_limit: Option<u64>,
        shared: bool,
        require_signatures: bool,
    ) -> PyResult<Stanchion> {
        let mut config = HostConfig {
            plugins,
            ..HostConfig::default()
        };
        config.sandbox.shared = shared;
        config.signatures.required = require_signatures;
        if let Some(libs) = libs {
            config.sandbox.libs = Some(libs);
        }
        if let Some(deny) = deny {
            config.sandbox.deny = Some(deny);
        }
        // `None` keeps the default ceiling; an explicit `0` is taken as "no limit",
        // since a zero-byte allowance would refuse every plugin.
        if let Some(bytes) = memory_limit {
            config.sandbox.memory_limit = (bytes > 0).then_some(bytes);
        }
        if let Some(instructions) = instruction_limit {
            config.sandbox.instruction_limit = (instructions > 0).then_some(instructions);
        }
        if let Some(allow) = allow {
            config.capabilities.allow = allow;
        }

        let mut builder = Host::builder().config(config);
        if let Some(capabilities) = capabilities {
            for (name, provider) in capabilities.iter() {
                let name: String = name.extract()?;
                builder = builder.capability(
                    name,
                    Arc::new(PyProvider {
                        callable: provider.unbind(),
                    }) as Arc<dyn CapabilityProvider>,
                );
            }
        }
        if let Some(policy) = policy {
            builder = builder.policy(Arc::new(PyPolicy { callable: policy }) as Arc<dyn FfiPolicy>);
        }

        Ok(Stanchion {
            inner: Arc::new(builder.build().map_err(raise)?),
        })
    }

    /// Reads a registry's configuration from a TOML file.
    ///
    /// The same shape `plugin-host` reads, so one policy file describes either
    /// transport. Capabilities and policy are still passed here, since neither is
    /// expressible in TOML.
    #[staticmethod]
    #[pyo3(signature = (path, *, capabilities = None, policy = None))]
    fn from_config_file(
        path: std::path::PathBuf,
        capabilities: Option<&Bound<'_, PyDict>>,
        policy: Option<Py<PyAny>>,
    ) -> PyResult<Stanchion> {
        let config = stanchion_ffi::load_config(&path)
            .map_err(|err| ConfigError::new_err(err.to_string()))?;

        let mut builder = Host::builder().config(config);
        if let Some(capabilities) = capabilities {
            for (name, provider) in capabilities.iter() {
                let name: String = name.extract()?;
                builder = builder.capability(
                    name,
                    Arc::new(PyProvider {
                        callable: provider.unbind(),
                    }) as Arc<dyn CapabilityProvider>,
                );
            }
        }
        if let Some(policy) = policy {
            builder = builder.policy(Arc::new(PyPolicy { callable: policy }) as Arc<dyn FfiPolicy>);
        }
        Ok(Stanchion {
            inner: Arc::new(builder.build().map_err(raise)?),
        })
    }

    /// Discovers and loads every plugin under a root.
    #[pyo3(signature = (root = None))]
    fn load(&self, py: Python<'_>, root: Option<std::path::PathBuf>) -> PyResult<LoadReport> {
        // The GIL goes back while Lua runs, so other Python threads keep going. A
        // capability provider re-acquires it on this same thread.
        let report = py
            .detach(|| self.inner.load(root.as_deref()))
            .map_err(raise)?;
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
    #[pyo3(signature = (root = None))]
    fn audit(&self, py: Python<'_>, root: Option<std::path::PathBuf>) -> PyResult<Vec<AuditEntry>> {
        let entries = py
            .detach(|| self.inner.audit(root.as_deref()))
            .map_err(raise)?;
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
    fn list(&self, py: Python<'_>) -> PyResult<Vec<PluginInfo>> {
        let plugins = py.detach(|| self.inner.list()).map_err(raise)?;
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
    fn names(&self, py: Python<'_>) -> PyResult<Vec<String>> {
        py.detach(|| self.inner.names()).map_err(raise)
    }

    /// Calls one method on one plugin.
    #[pyo3(signature = (plugin, method, *args))]
    fn call(
        &self,
        py: Python<'_>,
        plugin: &str,
        method: &str,
        args: &Bound<'_, PyTuple>,
    ) -> PyResult<Py<PyAny>> {
        let args = collect_args(args)?;
        let value = py
            .detach(|| self.inner.call(plugin, method, &args))
            .map_err(raise)?;
        Ok(to_py(py, &value)?.unbind())
    }

    /// Calls the same method on every plugin, collecting one result each.
    #[pyo3(signature = (method, *args))]
    fn dispatch(
        &self,
        py: Python<'_>,
        method: &str,
        args: &Bound<'_, PyTuple>,
    ) -> PyResult<Vec<Outcome>> {
        let args = collect_args(args)?;
        let outcomes = py
            .detach(|| self.inner.dispatch(method, &args))
            .map_err(raise)?;
        outcomes_to_py(py, outcomes)
    }

    /// Awaits one method on one plugin, which may yield.
    #[pyo3(signature = (plugin, method, *args))]
    fn call_async<'py>(
        &self,
        py: Python<'py>,
        plugin: String,
        method: String,
        args: &Bound<'_, PyTuple>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let args = collect_args(args)?;
        let host = Arc::clone(&self.inner);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let value = host
                .call_async(&plugin, &method, &args)
                .await
                .map_err(raise)?;
            Python::attach(|py| Ok(to_py(py, &value)?.unbind()))
        })
    }

    /// Awaits the same method on every plugin, in turn.
    #[pyo3(signature = (method, *args))]
    fn dispatch_async<'py>(
        &self,
        py: Python<'py>,
        method: String,
        args: &Bound<'_, PyTuple>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let args = collect_args(args)?;
        let host = Arc::clone(&self.inner);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let outcomes = host.dispatch_async(&method, &args).await.map_err(raise)?;
            Python::attach(|py| outcomes_to_py(py, outcomes))
        })
    }

    /// Re-reads one plugin from disk.
    fn reload(&self, py: Python<'_>, plugin: &str) -> PyResult<()> {
        py.detach(|| self.inner.reload(plugin)).map_err(raise)
    }

    /// Unbinds a granted capability from a live plugin.
    fn revoke(&self, py: Python<'_>, plugin: &str, capability: &str) -> PyResult<bool> {
        py.detach(|| self.inner.revoke(plugin, capability))
            .map_err(raise)
    }

    /// Whether plugins share one Lua state: `"shared"` or `"per-plugin"`.
    #[getter]
    fn isolation(&self, py: Python<'_>) -> PyResult<&'static str> {
        py.detach(|| self.inner.isolation()).map_err(raise)
    }

    fn __len__(&self, py: Python<'_>) -> PyResult<usize> {
        py.detach(|| self.inner.len()).map_err(raise)
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        Ok(format!("Stanchion({} plugins)", self.__len__(py)?))
    }
}

fn collect_args(args: &Bound<'_, PyTuple>) -> PyResult<Vec<Value>> {
    args.iter().map(|arg| from_py(&arg)).collect()
}

#[pymodule]
fn _stanchion(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<Stanchion>()?;
    module.add_class::<Decision>()?;
    module.add_class::<CapabilityCall>()?;
    module.add_class::<CapabilityRequest>()?;
    module.add_class::<LoadReport>()?;
    module.add_class::<Failure>()?;
    module.add_class::<PluginInfo>()?;
    module.add_class::<AuditEntry>()?;
    module.add_class::<Outcome>()?;

    module.add("StanchionError", module.py().get_type::<StanchionError>())?;
    module.add("UnknownPluginError", module.py().get_type::<UnknownPluginError>())?;
    module.add("PluginError", module.py().get_type::<PluginError>())?;
    module.add("LuaError", module.py().get_type::<LuaError>())?;
    module.add("ConfigError", module.py().get_type::<ConfigError>())?;
    module.add("CapabilityError", module.py().get_type::<CapabilityError>())?;
    module.add("ReentrantError", module.py().get_type::<ReentrantError>())?;
    Ok(())
}

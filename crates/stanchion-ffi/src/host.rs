//! The object a binding hands to its language: hosted plugins over ABI types.
//!
//! Hosts offer capabilities, set policy, and load plugin roots. Every plugin
//! — Lua, WASM, or a caller-registered runtime — loads through the same
//! [`stanchion_registry::Registry`]; this facade only translates its reports
//! into the flat shapes bindings consume.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use stanchion_abi::{AllowList, CapabilityProvider, Policy, Value};
use stanchion_lua::config::HostConfig;
use stanchion_registry::Registry;
use tokio::sync::{Mutex, MutexGuard};

use crate::error::{Error, Result};
use crate::guard::{CallGuard, next_id};

// The caller-facing report shapes live in `stanchion-abi`, so this crate and
// `stanchion-remote` cannot describe the same thing differently. They were defined
// here and again there, and had already drifted: `PluginInfo::runtime` existed only
// on this side.
pub use stanchion_abi::report::{AuditEntry, Failure, LoadReport, PluginInfo};

/// One plugin's result from a dispatch.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub plugin: String,
    /// Present when the call succeeded.
    pub value: Option<Value>,
    /// Present when it failed. One plugin failing never affects the others.
    pub error: Option<String>,
}

/// Shares one foreign policy across registry calls.
struct SharedPolicy(Arc<dyn Policy>);

impl Policy for SharedPolicy {
    fn decide(&self, request: &stanchion_abi::CapabilityRequest) -> stanchion_abi::Decision {
        self.0.decide(request)
    }
}

/// Collects everything a [`Stanchion`] needs before its first plugin loads.
///
/// Capabilities and policy cannot be changed afterwards, which is deliberate: a host
/// that can widen a plugin's reach after that plugin is running has given up the
/// guarantee the whole capability system exists to make.
#[derive(Default)]
pub struct Builder {
    config: HostConfig,
    policy: Option<Arc<dyn Policy>>,
    providers: BTreeMap<String, Arc<dyn CapabilityProvider>>,
    runtimes: Vec<Box<dyn stanchion_abi::Runtime>>,
}

impl Builder {
    /// A builder with the default sandbox: grouped states, restricted libraries.
    pub fn new() -> Self {
        Builder::default()
    }

    /// Replaces the whole configuration, as read from a host's TOML file.
    pub fn config(mut self, config: HostConfig) -> Self {
        self.config = config;
        self
    }

    /// Offers a capability, answered by foreign code.
    ///
    /// Offering is not granting: policy still decides, per plugin. A capability with
    /// no matching `allow` entry and no policy that grants it stays unreachable.
    pub fn capability(
        mut self,
        name: impl Into<String>,
        provider: Arc<dyn CapabilityProvider>,
    ) -> Self {
        let name = name.into();
        // A provider is no use to a plugin that is never granted the capability, and
        // a caller who registered one plainly means it to be reachable. Policy still
        // has the final say, and a supplied `Policy` overrides this entirely.
        if !self.config.capabilities.callbacks.contains(&name) {
            self.config.capabilities.callbacks.push(name.clone());
        }
        self.providers.insert(name, provider);
        self
    }

    /// Decides what each plugin may actually have.
    ///
    /// Without one, the allow-list in the configuration is the policy.
    pub fn policy(mut self, policy: Arc<dyn Policy>) -> Self {
        self.policy = Some(policy);
        self
    }

    /// Registers a runtime backend (e.g. WASM).
    ///
    /// Backends are selected by the manifest's `plugin_type` field. The
    /// built-in Lua backend is always registered; registering another
    /// backend for `lua` replaces it.
    pub fn runtime(mut self, runtime: Box<dyn stanchion_abi::Runtime>) -> Self {
        self.runtimes.push(runtime);
        self
    }

    /// Builds the host.
    pub fn build(self) -> Result<Stanchion> {
        let Builder {
            config,
            policy,
            providers,
            runtimes,
        } = self;

        let sandbox = config.sandbox.to_sandbox().map_err(Error::config)?;
        let isolation = if config.sandbox.shared {
            "shared"
        } else {
            "per-plugin"
        };
        let mut registry = Registry::new();
        if config.sandbox.shared {
            registry =
                registry.with_runtime(Box::new(stanchion_lua::backend::LuaBackend::shared()));
        } else {
            registry = registry
                .with_runtime(Box::new(stanchion_lua::backend::LuaBackend::isolated(sandbox)));
        }
        for runtime in runtimes {
            registry = registry.with_runtime(runtime);
        }

        registry = registry.with_setup(move |host| {
            for (name, provider) in providers {
                host.capability(name.clone(), provider);
            }
            Ok(())
        });

        let policy: Arc<dyn Policy> = policy.unwrap_or_else(|| {
            Arc::new(AllowList::new(
                config
                    .capabilities
                    .allow
                    .iter()
                    .chain(&config.capabilities.callbacks)
                    .cloned(),
            ))
        });
        registry = registry.with_policy(SharedPolicy(policy));

        #[cfg(feature = "signatures")]
        if config.signatures.required {
            registry = registry.require_signatures(true);
        }

        Ok(Stanchion {
            id: next_id(),
            registry: Arc::new(Mutex::new(registry)),
            default_root: config.plugins.clone(),
            isolation,
        })
    }
}

/// Hosted plugins, callable by name from any language.
///
/// Method names resolve when a call happens rather than when the plugin
/// loads — a binding is compiled long before anyone writes a plugin, so it
/// has no typed contract to offer.
pub struct Stanchion {
    /// Identifies this instance to the reentrancy guard.
    id: u64,
    registry: Arc<Mutex<Registry>>,
    default_root: Option<std::path::PathBuf>,
    isolation: &'static str,
}

/// Deliberately says nothing about the plugins: reading them would need the lock, and
/// a `Debug` impl that can block — or deadlock inside a provider — is a trap.
impl std::fmt::Debug for Stanchion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stanchion")
            .field("id", &self.id)
            .field("default_root", &self.default_root)
            .finish_non_exhaustive()
    }
}

impl Stanchion {
    /// Starts configuring a host.
    pub fn builder() -> Builder {
        Builder::new()
    }

    /// Claims the thread, then the lock — in that order.
    ///
    /// Re-entry has to be caught *before* blocking, or the diagnosis would be the
    /// hang it exists to prevent. Both guards are returned so they stay held
    /// for the whole call.
    fn enter(&self) -> Result<(CallGuard, MutexGuard<'_, Registry>)> {
        let guard = CallGuard::enter(self.id)?;
        let registry = futures_executor::block_on(self.registry.lock());
        Ok((guard, registry))
    }

    /// Discovers and loads every plugin under a root.
    pub fn load(&self, root: Option<&Path>) -> Result<LoadReport> {
        let root = self.root(root)?;
        let (_call, mut registry) = self.enter()?;
        // `load_dir` takes `&mut`: the borrow ends before the mapping below.
        let report = registry
            .load_dir(&root)
            .map_err(stanchion_abi::Error::from)?;
        Ok(LoadReport {
            loaded: report.loaded,
            failures: report
                .failures
                .into_iter()
                .map(|f| Failure {
                    plugin: f.name,
                    reason: f.reason.to_string(),
                })
                .collect(),
        })
    }

    /// Reports what every plugin under a root asks for, without running any of it.
    ///
    /// This is the call to make before `load` when the plugins are not yet trusted:
    /// it reads manifests and signatures only.
    pub fn audit(&self, root: Option<&Path>) -> Result<Vec<AuditEntry>> {
        let root = self.root(root)?;
        let (_call, registry) = self.enter()?;
        let audit = registry.audit(&root).map_err(stanchion_abi::Error::from)?;
        Ok(audit
            .plugins
            .iter()
            .map(|entry| AuditEntry {
                plugin: entry.name.clone(),
                capabilities: entry
                    .requests
                    .iter()
                    .map(|r| r.capability.clone())
                    .collect(),
                #[cfg(feature = "signatures")]
                signer: entry.signer.to_string(),
                #[cfg(not(feature = "signatures"))]
                signer: "unverified".to_string(),
            })
            .collect())
    }

    /// The loaded plugins.
    pub fn list(&self) -> Result<Vec<PluginInfo>> {
        let (_call, registry) = self.enter()?;
        Ok(registry
            .plugins()
            .iter()
            .map(|plugin| PluginInfo {
                name: plugin.name().to_string(),
                version: plugin.manifest().version.as_ref().map(ToString::to_string),
                granted: plugin.granted_capabilities().map(str::to_string).collect(),
                #[cfg(feature = "signatures")]
                signer: plugin.signer().to_string(),
                #[cfg(not(feature = "signatures"))]
                signer: "unverified".to_string(),
                runtime: plugin.instance().runtime().to_string(),
            })
            .collect())
    }

    /// The loaded plugins' names.
    pub fn names(&self) -> Result<Vec<String>> {
        let (_call, registry) = self.enter()?;
        Ok(registry.names().map(str::to_string).collect())
    }

    /// How many plugins are loaded.
    pub fn len(&self) -> Result<usize> {
        let (_call, registry) = self.enter()?;
        Ok(registry.len())
    }

    /// Whether no plugins are loaded.
    pub fn is_empty(&self) -> Result<bool> {
        Ok(self.len()? == 0)
    }

    /// Calls one method on one plugin.
    pub fn call(&self, plugin: &str, method: &str, args: &[Value]) -> Result<Value> {
        let _guard = CallGuard::enter(self.id)?;
        let registry = futures_executor::block_on(self.registry.lock());
        registry.call(plugin, method, args)
    }

    /// Calls the same method on every plugin, collecting one result each.
    ///
    /// A plugin that fails reports its error in place rather than ending the dispatch.
    pub fn dispatch(&self, method: &str, args: &[Value]) -> Result<Vec<Outcome>> {
        let _guard = CallGuard::enter(self.id)?;
        let registry = futures_executor::block_on(self.registry.lock());
        Ok(registry
            .dispatch(method, args)
            .into_iter()
            .map(|outcome| Outcome {
                plugin: outcome.plugin,
                value: outcome.value,
                error: outcome.error,
            })
            .collect())
    }

    /// Re-reads one plugin from disk.
    ///
    /// A plugin that fails to reload leaves the old instance in place.
    pub fn reload(&self, plugin: &str) -> Result<()> {
        let (_call, mut registry) = self.enter()?;
        registry.reload(plugin).map_err(stanchion_abi::Error::from)?;
        Ok(())
    }

    /// Unbinds a granted capability from a live plugin.
    ///
    /// Returns whether the plugin held it. Code that already captured the value in a
    /// local keeps it, so this defangs a misbehaving plugin without rewinding it.
    pub fn revoke(&self, plugin: &str, capability: &str) -> Result<bool> {
        let (_call, mut registry) = self.enter()?;
        registry
            .revoke(plugin, capability)
            .map_err(stanchion_abi::Error::from)
    }

    /// How plugin states relate to each other (`"shared"` or `"grouped"`).
    pub fn isolation(&self) -> Result<&'static str> {
        Ok(self.isolation)
    }

    /// The root to use, given what the caller passed and what was configured.
    fn root(&self, root: Option<&Path>) -> Result<std::path::PathBuf> {
        match (root, &self.default_root) {
            (Some(root), _) => Ok(root.to_path_buf()),
            (None, Some(configured)) => Ok(configured.clone()),
            (None, None) => Err(Error::Config(
                "no plugin root was given and none is configured".to_string(),
            )),
        }
    }
}

#[cfg(feature = "async")]
impl Stanchion {
    /// Awaits one method on one plugin.
    pub async fn call_async(&self, plugin: &str, method: &str, args: &[Value]) -> Result<Value> {
        let args = args.to_vec();
        let plugin = plugin.to_string();
        let method = method.to_string();
        crate::guard::Guarded::new(self.id, async {
            let registry = self.registry.lock().await;
            registry.call_async(&plugin, &method, &args).await
        })
        .await
    }

    /// Awaits the same method on every plugin, in turn.
    ///
    /// Sequential, not concurrent: backends sharing a state would only
    /// contend on it.
    pub async fn dispatch_async(&self, method: &str, args: &[Value]) -> Result<Vec<Outcome>> {
        let args = args.to_vec();
        let method = method.to_string();
        crate::guard::Guarded::new(self.id, async move {
            let registry = self.registry.lock().await;
            Ok(registry
                .dispatch_async(&method, &args)
                .await
                .into_iter()
                .map(|outcome| Outcome {
                    plugin: outcome.plugin,
                    value: outcome.value,
                    error: outcome.error,
                })
                .collect())
        })
        .await
    }
}

//! The object a binding hands to its language: a registry of dynamically-typed plugins.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

#[cfg(feature = "lua54")]
use mlua::{Lua, MultiValue};

use stanchion_registry::config::HostConfig;
#[allow(unused_imports)]
use stanchion_registry::{DynClass, DynInstance, Isolation, Plugin, PluginType, Registry};
#[allow(unused_imports)]
use stanchion_registry::DirectoryDigest;
#[allow(unused_imports)]
use stanchion_registry::Signer;
use tokio::sync::{Mutex, MutexGuard};

use crate::backend::{BackendRegistry, PluginBackend, PluginInstance};
use crate::budget::CallBudget;
use crate::callback::{AllowList, CapabilityCall, CapabilityProvider, Policy, PolicyBridge};
use crate::error::{Error, Result};
use crate::guard::{CallGuard, next_id};
use crate::value::Value;

/// What one `load` call did.
///
/// One plugin failing never stops the others, so this reports both halves rather than
/// returning at the first problem.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LoadReport {
    /// Names of plugins that loaded, in the order they were constructed.
    pub loaded: Vec<String>,
    /// Plugins that did not, and why.
    pub failures: Vec<Failure>,
}

impl LoadReport {
    /// True when every discovered plugin loaded.
    pub fn is_clean(&self) -> bool {
        self.failures.is_empty()
    }
}

/// One plugin that did not load.
#[derive(Clone, Debug, PartialEq)]
pub struct Failure {
    pub plugin: String,
    pub reason: String,
}

/// A loaded plugin, as a foreign caller sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct PluginInfo {
    pub name: String,
    pub version: Option<String>,
    pub granted: Vec<String>,
    pub signer: String,
    pub runtime: String,
}

/// What one plugin requests, established without executing any of its code.
#[derive(Clone, Debug, PartialEq)]
pub struct AuditEntry {
    pub plugin: String,
    pub capabilities: Vec<String>,
    pub signer: String,
}

/// One plugin's result from a dispatch.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub plugin: String,
    /// Present when the call succeeded.
    pub value: Option<Value>,
    /// Present when it failed. One plugin failing never affects the others.
    pub error: Option<String>,
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
    backends: BackendRegistry,
}

impl Builder {
    /// A builder with the default sandbox: per-plugin states, restricted libraries.
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

    /// Registers a non-Lua plugin backend (e.g. WASM).
    ///
    /// Backends are selected by the manifest's `plugin_type` field. The built-in
    /// Lua backend is always available; registering another backend for `lua` would
    /// replace it.
    pub fn backend(mut self, backend: Box<dyn PluginBackend>) -> Self {
        self.backends.register(backend);
        self
    }

    /// Builds the registry.
    pub fn build(self) -> Result<Stanchion> {
        let Builder {
            config,
            policy,
            providers,
            mut backends,
        } = self;

        let sandbox = config.sandbox.to_sandbox().map_err(Error::config)?;
        let mut registry = if config.sandbox.shared {
            Registry::new(stanchion_lua::new_lua())
        } else {
            Registry::isolated(stanchion_lua::new_lua(), sandbox)
        };

        let runtime = stanchion_lua::new_runtime();
        registry = registry.with_setup(move |host| {
            for (name, provider) in providers {
                let provider = Arc::clone(&provider);
                let capability = name.clone();
                host.capability(name.clone(), move |runtime, grant| {
                    let provider = Arc::clone(&provider);
                    let capability = capability.clone();
                    let plugin = grant.plugin().to_string();
                    let granted = crate::value::table_to_map(grant.params());
                    let lua = runtime
                        .lua_state()
                        .map(|arc| {
                            let guard = arc.lock().unwrap();
                            guard.clone()
                        })
                        .unwrap_or_else(stanchion_lua::new_lua);
                    let function = lua.create_function(move |lua, args: MultiValue| {
                        let mut converted = Vec::with_capacity(args.len());
                        for arg in args {
                            converted.push(crate::value::lua_to_abi(&lua, &arg));
                        }
                        let call = crate::callback::CapabilityCall {
                            plugin: plugin.clone(),
                            capability: capability.clone(),
                            grant: granted.clone(),
                            args: converted,
                        };
                        let answer = provider.invoke(&call).map_err(mlua::Error::RuntimeError)?;
                        crate::value::abi_to_lua(&answer, &lua)
                    })?;
                    Ok(mlua::Value::Function(function))
                });
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
        registry = registry.with_policy(PolicyBridge { inner: policy });

        #[cfg(feature = "signatures")]
        if config.signatures.required {
            registry = registry.require_signatures(true);
        }

        // Register the built-in Lua backend if no custom one was supplied.
        //
        // The same `Arc<Mutex<Registry>>` is shared between `self.registry`
        // (which `enter()` locks) and the `LuaBackend` (which `load()` uses
        // to wrap loaded plugins), so that every `Stanchion` method sees the
        // configured sandbox, capabilities, policy, and signature enforcement.
        if backends.get(&PluginType::Lua).is_none() {
            use crate::backend::LuaBackend;
            let registry = Arc::new(Mutex::new(registry));
            backends.register(Box::new(LuaBackend::new(Arc::clone(&runtime))));
            return Ok(Stanchion {
                id: next_id(),
                registry,
                backends: Mutex::new(backends),
                instances: Mutex::new(Vec::new()),
                default_root: config.plugins.clone(),
            });
        }

        Ok(Stanchion {
            id: next_id(),
            registry: Arc::new(Mutex::new(registry)),
            backends: Mutex::new(backends),
            instances: Mutex::new(Vec::new()),
            default_root: config.plugins.clone(),
        })
    }
}

/// A registry of Lua plugins, callable by name from any language.
///
/// Plugins load as `DynClass`: any table with a constructor, with method names
/// resolved when a call happens rather than when the plugin loads. A Rust host should
/// prefer a `#[lua_class]` trait, which checks the contract at load time — but a
/// binding is compiled long before anyone writes a plugin, so it has no trait to
/// offer, exactly as the out-of-process host does not.
pub struct Stanchion {
    /// Identifies this instance to the reentrancy guard.
    id: u64,
    /// The Lua registry, shared with the Lua backend so it can create instances.
    registry: Arc<Mutex<Registry<DynClass>>>,
    /// Registered non-Lua backends, keyed by plugin type.
    backends: Mutex<BackendRegistry>,
    /// Loaded non-Lua plugin instances.
    instances: Mutex<Vec<PluginInstanceEntry>>,
    default_root: Option<std::path::PathBuf>,
}

struct PluginInstanceEntry {
    name: String,
    instance: Box<dyn PluginInstance>,
    runtime: String,
    granted: Vec<String>,
    signer: String,
    digest: Option<String>,
    budget: Option<u64>,
    /// Per-call instruction meter for WASM / Lua backends.
    /// Consumed at call boundaries; `None` means unbounded.
    call_budget: Option<CallBudget>,
    dir: std::path::PathBuf,
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
    /// Starts configuring a registry.
    pub fn builder() -> Builder {
        Builder::new()
    }

    /// Claims the thread, then the lock — in that order.
    ///
    /// Re-entry has to be caught *before* blocking, or the diagnosis would be the
    /// hang it exists to prevent.
    fn enter(&self) -> Result<MutexGuard<'_, Registry<DynClass>>> {
        let _guard = CallGuard::enter(self.id)?;
        let registry = futures_executor::block_on(self.registry.lock());
        Ok(registry)
    }

    /// Discovers and loads every plugin under a root.
    ///
    /// Reads each manifest and dispatches to the appropriate backend based
    /// on `plugin_type`. Lua plugins go through the existing Lua registry;
    /// WASM (and other backend) plugins go through registered backends.
    pub fn load(&self, root: Option<&Path>) -> Result<LoadReport> {
        let root = self.root(root)?;
        let mut loaded = Vec::new();
        let mut failures = Vec::new();

        // Read every manifest.
        let (manifests, _discovery_failures) = stanchion_registry::discover(&root)
            .map_err(|e| Error::Io(format!("reading plugin root {:?}: {}", root, e)))?;

        // Separate Lua manifests from non-Lua manifests.
        let lua_names: Vec<String> = manifests
            .iter()
            .filter(|m| matches!(m.plugin_type, PluginType::Lua))
            .map(|m| m.name.clone())
            .collect();

        let non_lua_manifests: Vec<_> = manifests
            .into_iter()
            .filter(|m| !matches!(m.plugin_type, PluginType::Lua))
            .collect();

        // Load Lua plugins through the existing registry.
        if !lua_names.is_empty() {
            let mut registry = self.enter()?;
            match registry.load_dir(&root) {
                Ok(report) => {
                    loaded.extend(report.loaded);
                    failures.extend(report.failures.into_iter().map(|f| Failure {
                        plugin: f.name,
                        reason: f.reason.to_string(),
                    }));
                }
                Err(e) => {
                    for name in &lua_names {
                        failures.push(Failure {
                            plugin: name.clone(),
                            reason: e.to_string(),
                        });
                    }
                }
            }
        }

        // Load non-Lua plugins through registered backends.
        if !non_lua_manifests.is_empty() {
            // Acquire registry first to match lock ordering elsewhere (registry -> instances)
            // and to use it for signature/policy decisions.
            let registry = self.enter()?;
            let backends = futures_executor::block_on(self.backends.lock());
            let mut instances = futures_executor::block_on(self.instances.lock());

            for manifest in non_lua_manifests {
                let Some(backend) = backends.get(&manifest.plugin_type) else {
                    failures.push(Failure {
                        plugin: manifest.name.clone(),
                        reason: format!(
                            "no backend registered for plugin type '{:?}'",
                            manifest.plugin_type
                        ),
                    });
                    continue;
                };

                if let Err(e) = manifest.validate() {
                    failures.push(Failure {
                        plugin: manifest.name.clone(),
                        reason: e,
                    });
                    continue;
                }

                // Verify signature + directory (delegates to registry, same as Lua).
                #[cfg(feature = "signatures")]
                let (signer, digest) = match registry.verify_plugin(&manifest) {
                    Ok(pair) => pair,
                    Err(_) => {
                        failures.push(Failure {
                            plugin: manifest.name.clone(),
                            reason: "signature verification failed".to_string(),
                        });
                        continue;
                    }
                };
                #[cfg(not(feature = "signatures"))]
                let signer = Signer::Unsigned;
                #[cfg(not(feature = "signatures"))]
                let digest: Option<DirectoryDigest> = None;

                // Apply policy to determine which capabilities are granted.
                let granted = registry.evaluate_policy(
                    &manifest,
                    #[cfg(feature = "signatures")]
                    &signer,
                );

                // Derive per-call budget from manifest (Option<Badge>{max_instructions}).
                let budget = manifest.budget.as_ref().map(|b| b.max_instructions);
                let call_budget = budget.map(CallBudget::new);

                match backend.load(&manifest, &manifest.dir) {
                    Ok(instance) => {
                        let runtime = instance.runtime().to_string();
                        instances.push(PluginInstanceEntry {
                            name: manifest.name.clone(),
                            instance,
                            runtime,
                            granted,
                            signer: signer.to_string(),
                            digest: digest.as_ref().map(|d| d.hex().to_string()),
                            budget,
                            call_budget,
                            dir: manifest.dir.clone(),
                        });
                        loaded.push(manifest.name);
                    }
                    Err(e) => {
                        failures.push(Failure {
                            plugin: manifest.name,
                            reason: e.to_string(),
                        });
                    }
                }
            }
            drop(registry);
        }

        Ok(LoadReport { loaded, failures })
    }

    /// Reports what every plugin under a root asks for, without running any of it.
    ///
    /// This is the call to make before `load` when the plugins are not yet trusted:
    /// it reads manifests and signatures only.
    pub fn audit(&self, root: Option<&Path>) -> Result<Vec<AuditEntry>> {
        let root = self.root(root)?;
        let registry = self.enter()?;
        let audit = registry.audit(&root)?;
        Ok(audit
            .plugins
            .iter()
            .map(|entry| AuditEntry {
                plugin: entry.name.clone(),
                capabilities: entry.requests.iter().map(|r| r.name.clone()).collect(),
                signer: audit_signer(entry),
            })
            .collect())
    }

    /// The loaded plugins.
    pub fn list(&self) -> Result<Vec<PluginInfo>> {
        let registry = self.enter()?;
        let instances = futures_executor::block_on(self.instances.lock());

        let lua: Vec<PluginInfo> = registry.plugins().iter().map(|plugin| PluginInfo {
            name: plugin.name().to_string(),
            version: plugin.manifest().version.as_ref().map(ToString::to_string),
            granted: plugin.granted_capabilities().map(str::to_string).collect(),
            signer: plugin_signer(plugin),
            runtime: "lua".to_string(),
        }).collect();

        let non_lua: Vec<PluginInfo> = instances
            .iter()
            .map(|entry| PluginInfo {
                name: entry.name.clone(),
                version: None,
                granted: entry.granted.clone(),
                signer: entry.signer.clone(),
                runtime: entry.runtime.clone(),
            })
            .collect();

        let mut all = lua;
        all.extend(non_lua);
        Ok(all)
    }

    /// The loaded plugins' names.
    pub fn names(&self) -> Result<Vec<String>> {
        let registry = self.enter()?;
        let mut names: Vec<String> = registry.names().map(str::to_string).collect();
        drop(registry);
        let instances = futures_executor::block_on(self.instances.lock());
        names.extend(instances.iter().map(|e| e.name.clone()));
        Ok(names)
    }

    /// How many plugins are loaded.
    pub fn len(&self) -> Result<usize> {
        let registry = self.enter()?;
        let count = registry.len();
        drop(registry);
        let instances = futures_executor::block_on(self.instances.lock());
        Ok(count.saturating_add(instances.len()))
    }

    /// Whether no plugins are loaded.
    pub fn is_empty(&self) -> Result<bool> {
        Ok(self.len()? == 0)
    }

    /// Calls one method on one plugin.
    ///
    /// Lua plugins are looked up in the Lua registry; non-Lua plugins are
    /// dispatched through their backend instance.
    pub fn call(&self, plugin: &str, method: &str, args: &[Value]) -> Result<Value> {
        // Try Lua first (the common case).
        #[cfg(feature = "lua54")]
        {
            let guard = CallGuard::enter(self.id)?;
            let registry = futures_executor::block_on(self.registry.lock());
            if let Some(entry) = registry.get(plugin) {
                refresh_budget(entry);
                let result = call_plugin(entry.lua(), entry.instance(), method, args)?;
                return Ok(result);
            }
        }

        // Try non-Lua instances.
        let instances = futures_executor::block_on(self.instances.lock());
        for entry in instances.iter() {
            if entry.name == plugin {
                if let Some(budget) = &entry.call_budget {
                    budget.reset();
                }
                return entry.instance.call(method, args);
            }
        }
        drop(instances);

        Err(Error::UnknownPlugin(plugin.to_string()))
    }

    /// Calls the same method on every plugin (Lua then non-Lua), collecting one result each.
    ///
    /// A plugin that fails reports its error in place rather than ending the dispatch.
    pub fn dispatch(&self, method: &str, args: &[Value]) -> Result<Vec<Outcome>> {
        let _guard = CallGuard::enter(self.id)?;
        let mut outcomes = Vec::new();

        #[cfg(feature = "lua54")]
        {
            let registry = futures_executor::block_on(self.registry.lock());
            for plugin in registry.plugins() {
                refresh_budget(plugin);
                outcomes.push(match call_plugin(plugin.lua(), plugin.instance(), method, args) {
                    Ok(value) => Outcome {
                        plugin: plugin.name().to_string(),
                        value: Some(value),
                        error: None,
                    },
                    Err(err) => Outcome {
                        plugin: plugin.name().to_string(),
                        value: None,
                        error: Some(err.to_string()),
                    },
                });
            }
            drop(registry);
        }

        let instances = futures_executor::block_on(self.instances.lock());
        for entry in instances.iter() {
            if let Some(budget) = &entry.call_budget {
                budget.reset();
            }
            outcomes.push(match entry.instance.call(method, args) {
                Ok(value) => Outcome {
                    plugin: entry.name.clone(),
                    value: Some(value),
                    error: None,
                },
                Err(err) => Outcome {
                    plugin: entry.name.clone(),
                    value: None,
                    error: Some(err.to_string()),
                },
            });
        }

        Ok(outcomes)
    }

    /// Re-reads one plugin from disk.
    ///
    /// A plugin that fails to reload leaves the old instance in place.
    /// Supports both Lua (via Registry) and WASM/other backends (via instances).
    pub fn reload(&self, plugin: &str) -> Result<()> {
        // Try Lua registry first.
        {
            let mut registry = self.enter()?;
            match registry.reload(plugin) {
                Ok(()) => return Ok(()),
                Err(stanchion_registry::RegistryError::UnknownPlugin(_)) => {
                    // Fall through to non-Lua path.
                }
                Err(e) => return Err(e.into()),
            }
        }
        // Non-Lua path: find the instance and reload via its backend.
        // Acquire in registry -> backends -> instances order to avoid deadlock.
        let registry = self.enter()?;
        let backends = futures_executor::block_on(self.backends.lock());
        let instances = futures_executor::block_on(self.instances.lock());
        let dir = instances
            .iter()
            .find(|e| e.name == plugin)
            .map(|e| e.dir.clone())
            .ok_or_else(|| Error::UnknownPlugin(plugin.to_string()))?;
        // Read fresh manifest and validate (filesystem, no lock needed).
        // Clone dir to release borrow before reading.
        drop(instances);
        drop(backends);
        // Registry already held for verify; we need to keep it but we dropped
        // backends/instances to avoid holding them during IO. Re-acquire after read.
        let manifest = stanchion_registry::read_manifest(&dir)
            .map_err(|e| Error::Io(format!("reading manifest for reload of '{}': {}", plugin, e)))?;
        if manifest.name != plugin {
            return Err(Error::Config(format!(
                "manifest renamed the plugin to `{}`; remove and load it again instead",
                manifest.name
            )));
        }
        if let Err(e) = manifest.validate() {
            return Err(Error::Config(e));
        }
        // Re-acquire backends and instances in correct order after IO.
        // Registry is still held from above; now re-lock backends then instances.
        let backends = futures_executor::block_on(self.backends.lock());
        let mut instances = futures_executor::block_on(self.instances.lock());
        #[cfg(feature = "signatures")]
        let (signer, digest) = registry
            .verify_plugin(&manifest)
            .map_err(|r| Error::Io(format!("signature verification failed for reload of '{}': {}", plugin, r)))?;
        #[cfg(not(feature = "signatures"))]
        let signer = Signer::Unsigned;
        #[cfg(not(feature = "signatures"))]
        let digest: Option<DirectoryDigest> = None;
        let granted = registry.evaluate_policy(
            &manifest,
            #[cfg(feature = "signatures")]
            &signer,
        );
        let budget = manifest.budget.as_ref().map(|b| b.max_instructions);
        let call_budget = budget.map(CallBudget::new);
        let Some(backend) = backends.get(&manifest.plugin_type) else {
            return Err(Error::Config(format!(
                "no backend registered for plugin type '{:?}'",
                manifest.plugin_type
            )));
        };
        let new_instance = backend
            .load(&manifest, &manifest.dir)
            .map_err(|e| Error::Io(format!("reload failed for '{}': {}", plugin, e)))?;
        let runtime = new_instance.runtime().to_string();
        let entry = instances
            .iter_mut()
            .find(|e| e.name == plugin)
            .ok_or_else(|| Error::UnknownPlugin(plugin.to_string()))?;
        entry.instance = new_instance;
        entry.runtime = runtime;
        entry.granted = granted;
        entry.signer = signer.to_string();
        entry.digest = digest.as_ref().map(|d| d.hex().to_string());
        entry.budget = budget;
        entry.call_budget = call_budget;
        entry.dir = manifest.dir;
        Ok(())
    }

    /// Unbinds a granted capability from a live plugin.
    ///
    /// Returns whether the plugin held it. Code that already captured the value in a
    /// local keeps it, so this defangs a misbehaving plugin without rewinding it.
    pub fn revoke(&self, plugin: &str, capability: &str) -> Result<bool> {
        // Try Lua registry first; UnknownPlugin falls through to WASM.
        {
            let mut registry = self.enter()?;
            match registry.revoke(plugin, capability) {
                Ok(held) => return Ok(held),
                Err(stanchion_registry::RegistryError::UnknownPlugin(_)) => {}
                Err(e) => return Err(e.into()),
            }
        }
        let mut instances = futures_executor::block_on(self.instances.lock());
        for entry in instances.iter_mut() {
            if entry.name == plugin {
                if let Some(idx) = entry.granted.iter().position(|c| c == capability) {
                    entry.granted.remove(idx);
                    return Ok(true);
                } else {
                    return Ok(false);
                }
            }
        }
        Err(Error::UnknownPlugin(plugin.to_string()))
    }

    /// Whether plugins share one Lua state.
    pub fn isolation(&self) -> Result<&'static str> {
        let registry = self.enter()?;
        Ok(match registry.isolation() {
            Isolation::Shared => "shared",
            Isolation::PerPlugin(_) => "per-plugin",
            Isolation::PerGroup(_) => "per-group",
        })
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
    ///
    /// The plugin's method runs as a coroutine, so it can yield — a Lua `async fn`
    /// that waits on the host does not block the state it runs in.
    pub async fn call_async(&self, plugin: &str, method: &str, args: &[Value]) -> Result<Value> {
        let args = args.to_vec();
        let plugin = plugin.to_string();
        let method = method.to_string();

        // Try Lua first.
        let result: std::result::Result<Value, crate::Error> = crate::guard::Guarded::new(self.id, async {
            let registry = self.registry.lock().await;
            let entry = registry
                .get(&plugin)
                .ok_or_else(|| Error::UnknownPlugin(plugin.clone()))?;
            refresh_budget(entry);
            let lua_args = to_lua_args(entry.lua(), &args)?;
            let result = entry.instance().call_method_async(&method, lua_args).await
                .map_err(|e| Error::Runtime(stanchion_abi::RuntimeError::from(e)))?;
            Ok(crate::value::lua_to_abi(entry.lua(), &result))
        }).await;

        match result {
            Ok(value) => return Ok(value),
            Err(Error::UnknownPlugin(_)) => {}
            Err(e) => return Err(e),
        }

        // Try non-Lua instances.
        let instances = self.instances.lock().await;
        for entry in instances.iter() {
            if entry.name == plugin {
                if let Some(budget) = &entry.call_budget {
                    budget.reset();
                }
                return entry.instance.call(&method, &args);
            }
        }
        drop(instances);

        Err(Error::UnknownPlugin(plugin))
    }

    /// Awaits the same method on every plugin, in turn.
    ///
    /// Sequential, not concurrent: under shared isolation every plugin reaches the
    /// same Lua state, so running them together would only contend on it.
    pub async fn dispatch_async(&self, method: &str, args: &[Value]) -> Result<Vec<Outcome>> {
        let args = args.to_vec();
        let method = method.to_string();
        crate::guard::Guarded::new(self.id, async move {
            let mut outcomes = Vec::new();

            // Lua plugins
            let registry = self.registry.lock().await;
            for plugin in registry.plugins() {
                refresh_budget(plugin);
                let outcome = match to_lua_args(plugin.lua(), &args) {
                    Ok(lua_args) => plugin
                        .instance()
                        .call_method_async(&method, lua_args)
                        .await
                        .map_err(|e| Error::Runtime(stanchion_abi::RuntimeError::from(e)))
                        .and_then(|value| Ok(crate::value::lua_to_abi(plugin.lua(), &value))),
                    Err(err) => Err(err),
                };
                outcomes.push(match outcome {
                    Ok(value) => Outcome {
                        plugin: plugin.name().to_string(),
                        value: Some(value),
                        error: None,
                    },
                    Err(err) => Outcome {
                        plugin: plugin.name().to_string(),
                        value: None,
                        error: Some(err.to_string()),
                    },
                });
            }
            drop(registry);

            // Non-Lua plugins
            let instances = self.instances.lock().await;
            for entry in instances.iter() {
                if let Some(budget) = &entry.call_budget {
                    budget.reset();
                }
                outcomes.push(match entry.instance.call(&method, &args) {
                    Ok(value) => Outcome {
                        plugin: entry.name.clone(),
                        value: Some(value),
                        error: None,
                    },
                    Err(err) => Outcome {
                        plugin: entry.name.clone(),
                        value: None,
                        error: Some(err.to_string()),
                    },
                });
            }

            Ok(outcomes)
        })
        .await
    }
}

/// Converts arguments into the state the plugin actually runs in.
///
/// Under per-plugin isolation each plugin has its own [`Lua`], and a value built in
/// one state cannot be passed to another — so this happens per plugin rather than
/// once per dispatch.
#[cfg(feature = "lua54")]
    fn to_lua_args(lua: &Lua, args: &[Value]) -> stanchion_abi::Result<MultiValue> {
        let mut converted = Vec::with_capacity(args.len());
        for arg in args {
            converted.push(
                crate::value::abi_to_lua(arg, lua)
                    .map_err(|e| crate::Error::Runtime(stanchion_abi::RuntimeError::from(e)))?
            );
        }
        Ok(MultiValue::from_iter(converted))
    }

    #[cfg(feature = "lua54")]
    fn call_plugin(lua: &Lua, instance: &DynInstance, method: &str, args: &[Value]) -> stanchion_abi::Result<Value> {
        let result = instance
            .call_method(method, to_lua_args(lua, args)?)
            .map_err(|e| crate::Error::Runtime(stanchion_abi::RuntimeError::from(e)))?;
        Ok(crate::value::lua_to_abi(lua, &result))
    }

    /// Gives a plugin its full instruction allowance back.
    ///
/// The limit is documented as applying per call rather than per plugin lifetime, and
/// `Registry::dispatch` resets it for exactly that reason. Without this a long-lived
/// plugin would eventually exhaust its budget and never recover.
fn refresh_budget(plugin: &Plugin<DynClass>) {
    if let Some(budget) = plugin.budget() {
        budget.reset();
    }
}

#[cfg(feature = "signatures")]
fn plugin_signer(plugin: &Plugin<DynClass>) -> String {
    plugin.signer().to_string()
}

#[cfg(not(feature = "signatures"))]
fn plugin_signer(_plugin: &Plugin<DynClass>) -> String {
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

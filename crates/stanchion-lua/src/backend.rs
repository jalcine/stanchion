//! The Lua backend: states, loading, calling, reload and revocation.
//!
//! Everything in this module speaks `mlua` so nothing else has to. Hosts
//! hold a [`LuaBackend`] behind [`stanchion_abi::Runtime`] and pass
//! [`stanchion_abi::Value`]s; conversion happens at this boundary.
//!
//! # States
//!
//! A [`SharedState`] is one `Lua` plus its instruction budget. Plugins that
//! must exchange Lua values (a dependency chain) share one; everything else
//! is kept apart according to the backend's [`IsolationMode`]. The pool maps
//! plugin names to their state, so reload reuses the state a plugin already
//! runs in.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use mlua::{Lua, LuaSerdeExt, MultiValue, ObjectLike, Table, Value as LuaValue};
use stanchion_abi::{
    CapabilityCall, CapabilityRequest, Decision, Grant, HostSetup, Manifest, PluginBackend,
    PluginInstance, PluginType, ResourceLimits, Result as AbiResult, Runtime, Value,
    Error as AbiError, GroupOutcome, LoadContext, LoadItem, RuntimeError,
};
use stanchion_abi::{rocks::RockPaths, signature::DirectoryDigest};

use crate::convert::{self, abi_to_lua, lua_to_abi};
use crate::sandbox::{Budget, Sandbox};

/// How plugin states relate to each other.
///
/// Two plugins share a state exactly when a chain of `[dependencies]`
/// connects them, because that chain is what carries Lua values between
/// them. Memory and instruction limits are properties of a state, so the
/// group — not the plugin — is the accounting unit wherever states are shared.
#[derive(Debug, Clone)]
pub enum IsolationMode {
    /// Every plugin runs in one shared state.
    Shared,
    /// Every plugin gets its own state built under a [`Sandbox`] policy.
    ///
    /// Values cannot cross states, so a `[dependencies]` entry fails to load.
    PerPlugin(Sandbox),
    /// One state per dependency group, built under a [`Sandbox`] policy.
    PerGroup(Sandbox),
}

/// Maps an `mlua` error into a backend failure string.
fn mlua_err<T>(result: mlua::Result<T>) -> std::result::Result<T, String> {
    result.map_err(|e| e.to_string())
}

/// Key a plugin publishes its public surface under.
pub const EXPORTS_KEY: &str = "exports";

/// Constructor looked up on a plugin's class table when none is configured.
pub const DEFAULT_CONSTRUCTOR: &str = "new";

/// Largest module file `require` will read, in bytes.
///
/// A cap matters because the search is over host-readable paths: without it
/// a file such as `/dev/zero` (were it ever reachable) would read without
/// bound in Rust, outside the Lua memory limit. 8 MiB is far above any
/// realistic Lua module.
const MAX_MODULE_BYTES: u64 = 8 * 1024 * 1024;

/// One `Lua` plus its instruction budget, shared by a dependency group.
struct SharedState {
    lua: Mutex<Lua>,
    budget: Option<Budget>,
}

impl SharedState {
    fn reset_budget(&self) {
        if let Some(budget) = &self.budget {
            budget.reset();
        }
    }
}

/// A loaded Lua plugin: its handle plus the state it runs in.
pub struct LuaInstance {
    state: Arc<SharedState>,
    handle: crate::LuaHandle,
    env: Table,
}

impl LuaInstance {
    /// Drives a method that may yield, resuming its coroutine to completion.
    ///
    /// The state lock is never held across an await (its guard is not
    /// `Send`): arguments convert under a short lock, the coroutine runs
    /// unlocked, and the result converts under a second one. Hosts
    /// serialize concurrent calls on shared states themselves — the FFI
    /// host holds its registry lock for the whole dispatch.
    #[cfg(feature = "async")]
    async fn call_async_inner(&self, method: &str, args: &[Value]) -> AbiResult<Value> {
        let poisoned = || {
            AbiError::Runtime(RuntimeError {
                runtime_name: "lua".to_string(),
                error: "the Lua state is poisoned".to_string(),
            })
        };
        let converted = {
            let lua = self.state.lua.lock().map_err(|_| poisoned())?;
            self.state.reset_budget();
            let mut converted = MultiValue::new();
            for arg in args {
                converted.push_back(abi_to_lua(arg, &lua).map_err(|e| {
                    AbiError::Runtime(RuntimeError {
                        runtime_name: "lua".to_string(),
                        error: e.to_string(),
                    })
                })?);
            }
            converted
        };
        // Panics during resumption are caught by the host's poll-level
        // guard; a synchronous guard cannot wrap the await itself.
        let result: LuaValue = self
            .handle
            .call_async_method(method, converted)
            .await
            .map_err(|e| {
                AbiError::Runtime(RuntimeError {
                    runtime_name: "lua".to_string(),
                    error: e.to_string(),
                })
            })?;
        let lua = self.state.lua.lock().map_err(|_| poisoned())?;
        Ok(lua_to_abi(&lua, &result))
    }

    /// Synchronous methods run through the ordinary call path.
    #[cfg(not(feature = "async"))]
    async fn call_async_inner(&self, method: &str, args: &[Value]) -> AbiResult<Value> {
        self.call(method, args)
    }

    fn call_guarded(&self, method: &str, args: &[Value]) -> AbiResult<Value> {
        let lua = self.state.lua.lock().map_err(|_| {
            AbiError::Runtime(RuntimeError {
                runtime_name: "lua".to_string(),
                error: "the Lua state is poisoned".to_string(),
            })
        })?;
        // The limit applies per call rather than per plugin lifetime.
        self.state.reset_budget();
        let result = self.call_locked(&lua, method, args).map_err(|e| {
            AbiError::Runtime(RuntimeError {
                runtime_name: "lua".to_string(),
                error: e,
            })
        })?;
        Ok(lua_to_abi(&lua, &result))
    }

    fn call_locked(
        &self,
        lua: &Lua,
        method: &str,
        args: &[Value],
    ) -> std::result::Result<LuaValue, String> {
        let mut converted = MultiValue::new();
        for arg in args {
            converted.push_back(abi_to_lua(arg, lua).map_err(|e| e.to_string())?);
        }
        self.handle
            .call_method(method, converted)
            .map_err(|e| e.to_string())
    }
}

impl PluginInstance for LuaInstance {
    fn call(&self, method: &str, args: &[Value]) -> AbiResult<Value> {
        // A panicking plugin is reported like a failing one rather than
        // taking the caller's stack with it.
        stanchion_abi::panics::guard(|| self.call_guarded(method, args))
            .map_err(|panicked| {
                AbiError::Runtime(RuntimeError {
                    runtime_name: "lua".to_string(),
                    error: panicked.to_string(),
                })
            })?
    }

    fn runtime(&self) -> &str {
        "lua"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn call_async(
        &self,
        method: &str,
        args: &[Value],
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = AbiResult<Value>> + Send + '_>,
    > {
        let method = method.to_string();
        let args = args.to_vec();
        Box::pin(async move { self.call_async_inner(&method, &args).await })
    }
}

/// The Lua backend: builds states under a [`Sandbox`] and loads plugins.
///
/// Holds a pool of states keyed by plugin name so reload reuses the state a
/// plugin already runs in. All interior mutability is behind mutexes; a
/// `Lua` itself is driven under lock, one call at a time.
pub struct LuaBackend {
    mode: IsolationMode,
    constructor: String,
    states: Mutex<HashMap<String, Arc<SharedState>>>,
    /// Live export surfaces by plugin name: proxy plus its metatable, so
    /// reload can repoint dependents at the new surface.
    surfaces: Mutex<HashMap<String, (Table, Table)>>,
}

impl LuaBackend {
    /// Every plugin shares one state. No per-plugin limits are possible —
    /// they are properties of a state — so prefer [`LuaBackend::isolated`]
    /// for code you did not write.
    pub fn shared() -> Self {
        LuaBackend {
            mode: IsolationMode::Shared,
            constructor: DEFAULT_CONSTRUCTOR.to_string(),
            states: Mutex::new(HashMap::new()),
            surfaces: Mutex::new(HashMap::new()),
        }
    }

    /// Every plugin gets its own state under `sandbox`.
    pub fn isolated(sandbox: Sandbox) -> Self {
        LuaBackend {
            mode: IsolationMode::PerPlugin(sandbox),
            constructor: DEFAULT_CONSTRUCTOR.to_string(),
            states: Mutex::new(HashMap::new()),
            surfaces: Mutex::new(HashMap::new()),
        }
    }

    /// One state per dependency group under `sandbox`.
    pub fn grouped(sandbox: Sandbox) -> Self {
        LuaBackend {
            mode: IsolationMode::PerGroup(sandbox),
            constructor: DEFAULT_CONSTRUCTOR.to_string(),
            states: Mutex::new(HashMap::new()),
            surfaces: Mutex::new(HashMap::new()),
        }
    }

    /// Uses a different constructor name than [`DEFAULT_CONSTRUCTOR`].
    pub fn with_constructor(mut self, name: impl Into<String>) -> Self {
        self.constructor = name.into();
        self
    }

    /// Applies backend-neutral [`ResourceLimits`] as an isolation policy.
    ///
    /// Limits are properties of a Lua state, so they need one state per
    /// plugin (or group) to mean anything; `shared` mode keeps a single
    /// state and the limits apply to the whole host.
    pub fn with_limits(sandbox: Sandbox, limits: ResourceLimits, shared: bool) -> Self {
        let mut sandbox = sandbox;
        if let Some(bytes) = limits.memory_bytes {
            sandbox = sandbox.memory_limit(bytes);
        }
        if let Some(instructions) = limits.max_instructions {
            sandbox = sandbox.instruction_limit(instructions);
        }
        if shared {
            LuaBackend {
                mode: IsolationMode::Shared,
                constructor: DEFAULT_CONSTRUCTOR.to_string(),
                states: Mutex::new(HashMap::new()),
                surfaces: Mutex::new(HashMap::new()),
            }
        } else {
            LuaBackend::grouped(sandbox)
        }
    }

    /// How states are allocated, for hosts that attest their posture.
    pub fn isolation_name(&self) -> &'static str {
        match self.mode {
            IsolationMode::Shared => "shared",
            IsolationMode::PerPlugin(_) => "per-plugin",
            IsolationMode::PerGroup(_) => "per-group",
        }
    }

    /// Creates the state for one isolation unit, binding ambient values.
    fn create_state(&self, setup: &HostSetup, rock_paths: Option<&RockPaths>) -> mlua::Result<SharedState> {
        let (lua, budget) = match &self.mode {
            IsolationMode::Shared => {
                // The shared state is created once and reused; ambient
                // values are bound at creation below.
                let (lua, budget) = Sandbox::restricted().build()?;
                (lua, budget)
            }
            IsolationMode::PerPlugin(sandbox) | IsolationMode::PerGroup(sandbox) => {
                sandbox.build()?
            }
        };
        // Ambient authority reaches every state, gated by nothing.
        for (label, value) in setup.ambient_values() {
            let bound = abi_to_lua(value, &lua)?;
            lua.globals().set(label, bound)?;
        }
        if let Some(paths) = rock_paths {
            prepend_module_paths(&lua, paths)?;
        }
        Ok(SharedState {
            lua: Mutex::new(lua),
            budget,
        })
    }

    /// The state `name` runs in, creating and recording it when missing.
    fn state_for(
        &self,
        name: &str,
        group: u64,
        group_states: &mut HashMap<u64, Arc<SharedState>>,
        setup: &HostSetup,
        rock_paths: Option<&RockPaths>,
    ) -> mlua::Result<Arc<SharedState>> {
        // A state recorded by an earlier load (or reload) wins: reload must
        // reuse the state dependents already hold proxies into.
        if let Some(state) = self.states.lock().as_ref().ok().and_then(|pool| pool.get(name).cloned()) {
            return Ok(state);
        }
        let key = match self.mode {
            IsolationMode::Shared => 0,
            IsolationMode::PerPlugin(_) => {
                // Fresh states are recorded per plugin below; the transient
                // map is unused in this mode.
                let state = Arc::new(self.create_state(setup, rock_paths)?);
                if let Ok(mut pool) = self.states.lock() {
                    pool.insert(name.to_string(), Arc::clone(&state));
                }
                return Ok(state);
            }
            IsolationMode::PerGroup(_) => group,
        };
        if let Some(state) = group_states.get(&key) {
            if let Ok(mut pool) = self.states.lock() {
                pool.insert(name.to_string(), Arc::clone(state));
            }
            return Ok(Arc::clone(state));
        }
        let state = Arc::new(self.create_state(setup, rock_paths)?);
        group_states.insert(key, Arc::clone(&state));
        if let Ok(mut pool) = self.states.lock() {
            pool.insert(name.to_string(), Arc::clone(&state));
        }
        Ok(state)
    }

    /// Loads one verified item into its state.
    #[allow(clippy::too_many_arguments)]
    fn load_one(
        &self,
        state: &Arc<SharedState>,
        item: &LoadItem,
        ctx: &LoadContext,
        rock_templates: &[String],
        dependencies: HashMap<String, Table>,
    ) -> std::result::Result<(LuaInstance, Vec<String>), String> {
        let manifest = item.manifest;
        let lua = state.lua.lock().map_err(|_| "the Lua state is poisoned".to_string())?;

        let environment = mlua_err(plugin_environment(&lua))?;
        let granted = self.grant_capabilities(&lua, &environment, manifest, &item.signer, ctx)?;

        install_plugin_require(
            &lua,
            &environment,
            true,
            rock_templates.to_vec(),
            manifest.dir.clone(),
            item.digest.clone(),
        )
        .map_err(|e| e.to_string())?;

        state.reset_budget();

        let name = manifest.entry_path().display().to_string();
        let chunk = lua
            .load(&item.entry_bytes)
            .set_name(name)
            .set_environment(environment.clone());
        let class: LuaValue = mlua_err(chunk.eval())?;
        let handle = mlua_err(crate::__private::expect_handle(class, "plugin class"))?;

        let config = mlua_err(lua.to_value(&manifest.config))?;
        let deps = mlua_err(lua.create_table())?;
        for (dep_name, proxy) in &dependencies {
            mlua_err(deps.set(dep_name.as_str(), proxy.clone()))?;
        }
        // Optional dependencies that never loaded simply leave a nil slot:
        // only present members are set above.
        let instance_value: LuaValue = match &handle {
            crate::LuaHandle::Table(table) => {
                mlua_err(table.call_function(ctx.constructor, (config, deps.clone())))?
            }
            crate::LuaHandle::UserData(data) => {
                mlua_err(data.call_function(ctx.constructor, (config, deps.clone())))?
            }
        };
        let instance_handle = mlua_err(crate::__private::expect_handle(
            instance_value,
            "plugin instance",
        ))?;

        Ok((
            LuaInstance {
                state: Arc::clone(state),
                handle: instance_handle,
                env: environment,
            },
            granted,
        ))
    }

    /// Resolves and binds one plugin's declared capabilities into its env.
    fn grant_capabilities(
        &self,
        lua: &Lua,
        environment: &Table,
        manifest: &Manifest,
        signer: &stanchion_abi::signature::Signer,
        ctx: &LoadContext,
    ) -> std::result::Result<Vec<String>, String> {
        let mut granted = Vec::new();
        for (name, declared) in &manifest.capabilities {
            let (params, optional) = stanchion_abi::callback::split_optional(declared);
            let request = CapabilityRequest {
                plugin: manifest.name.clone(),
                capability: name.clone(),
                params,
                optional,
                signer: signer.to_string(),
            };
            let Some(provider) = ctx.setup.provider(name) else {
                if optional {
                    continue;
                }
                return Err(format!(
                    "requests capability `{name}`, which the host does not offer"
                ));
            };
            // Deny by default: offering a capability is not granting it.
            let approved = match ctx.policy.decide(&request) {
                Decision::Grant => request.params.clone(),
                Decision::GrantWith(params) => {
                    if !matches!(params, Value::Map(_)) {
                        return Err(format!(
                            "policy granted `{name}` with a non-table value"
                        ));
                    }
                    params
                }
                Decision::Deny(reason) => {
                    if optional {
                        continue;
                    }
                    return Err(format!("capability `{name}` denied: {reason}"));
                }
            };
            let grant = Grant::new(manifest.name.clone(), name.clone(), approved);
            let function = mlua_err(bind_provider(lua, provider, &grant))?;
            environment
                .set(name.as_str(), function)
                .map_err(|e| e.to_string())?;
            granted.push(name.clone());
        }
        Ok(granted)
    }
}

/// Binds a provider as a Lua function capturing its approved grant.
fn bind_provider(
    lua: &Lua,
    provider: &Arc<dyn stanchion_abi::CapabilityProvider>,
    grant: &Grant,
) -> mlua::Result<LuaValue> {
    let provider = Arc::clone(provider);
    let plugin = grant.plugin().to_string();
    let capability = grant.name().to_string();
    let granted = grant.params().clone();
    let function = lua.create_function(move |lua, args: MultiValue| {
        let mut converted = Vec::with_capacity(args.len());
        for arg in args {
            converted.push(convert::lua_to_abi(lua, &arg));
        }
        let call = CapabilityCall {
            plugin: plugin.clone(),
            capability: capability.clone(),
            grant: granted.clone(),
            args: converted,
        };
        let answer = provider.invoke(&call).map_err(mlua::Error::RuntimeError)?;
        convert::abi_to_lua(&answer, lua)
    })?;
    Ok(LuaValue::Function(function))
}

/// A table that reads through to the real globals but keeps writes local.
fn plugin_environment(lua: &Lua) -> mlua::Result<Table> {
    let environment = lua.create_table()?;
    let metatable = lua.create_table()?;
    metatable.set("__index", lua.globals())?;
    environment.set_metatable(Some(metatable))?;

    // Under shared/grouped isolation every environment `__index`es the same
    // globals, so each gets its own shallow copy of the mutable core library
    // tables as *own* fields and field reassignment stays local.
    let globals = lua.globals();
    for name in ["string", "table", "math", "coroutine", "os", "io"] {
        if let Some(lib) = globals.get::<Option<Table>>(name)? {
            environment.set(name, shallow_copy(lua, &lib)?)?;
        }
    }
    Ok(environment)
}

/// Copies a table's own key/value pairs into a fresh table (values shared).
fn shallow_copy(lua: &Lua, table: &Table) -> mlua::Result<Table> {
    let copy = lua.create_table()?;
    for pair in table.clone().pairs::<LuaValue, LuaValue>() {
        let (key, value) = pair?;
        copy.set(key, value)?;
    }
    Ok(copy)
}

/// Prepends a rocks tree's search paths to the shared state.
fn prepend_module_paths(lua: &Lua, paths: &RockPaths) -> mlua::Result<()> {
    let Some(package) = lua.globals().get::<Option<Table>>("package")? else {
        return Ok(());
    };
    for (key, addition) in [("path", Some(&paths.path)), ("cpath", paths.cpath.as_ref())] {
        let Some(addition) = addition else { continue };
        if addition.is_empty() {
            continue;
        }
        let current: String = package.get(key).unwrap_or_default();
        if !current.contains(addition.as_str()) {
            package.set(key, format!("{addition};{current}"))?;
        }
    }
    Ok(())
}

/// Gives a plugin a `require` that loads its modules into its own env.
///
/// Search templates are fixed at load time, never read from the live,
/// plugin-writable `package.path`. Each candidate is canonicalised and must
/// be a regular file under its template's directory; symlinks out, devices
/// and oversized files are refused. Anything unresolvable falls through to
/// the original `require` only when not restricted.
fn install_plugin_require(
    lua: &Lua,
    environment: &Table,
    restricted: bool,
    search_templates: Vec<String>,
    plugin_dir: std::path::PathBuf,
    digest: Option<DirectoryDigest>,
) -> mlua::Result<()> {
    let loaded = lua.create_table()?;
    let fallback: Option<mlua::Function> = lua.globals().get("require")?;
    let plugin_env = environment.clone();

    let roots: Vec<Option<std::path::PathBuf>> = search_templates
        .iter()
        .map(|template| template_root(template))
        .collect();

    let plugin_dir = std::fs::canonicalize(&plugin_dir).unwrap_or(plugin_dir);

    let require = lua.create_function(move |lua, name: String| {
        if let Some(cached) = loaded.get::<Option<LuaValue>>(name.as_str())? {
            return Ok(cached);
        }

        // Check environment table first for capabilities bound by grant_capabilities
        if let Ok(Some(cap)) = plugin_env.get::<Option<LuaValue>>(name.as_str()) {
            loaded.set(name.as_str(), cap.clone())?;
            return Ok(cap);
        }

        let relative = name.replace('.', std::path::MAIN_SEPARATOR_STR);

        for (template, root) in search_templates.iter().zip(roots.iter()) {
            let candidate = template.replace('?', &relative);

            let Ok(resolved) = std::fs::canonicalize(&candidate) else {
                continue;
            };
            let Ok(meta) = std::fs::metadata(&resolved) else {
                continue;
            };
            if !meta.is_file() || meta.len() > MAX_MODULE_BYTES {
                continue;
            }
            match root {
                Some(root) if resolved.starts_with(root) => {}
                _ => continue,
            }

            let Ok(source) = std::fs::read_to_string(&resolved) else {
                continue;
            };

            if let Some(digest) = &digest
                && let Ok(relative) = resolved.strip_prefix(&plugin_dir)
            {
                let relative = slash_path(relative);
                if !digest.covers(&relative) {
                    return Err(mlua::Error::RuntimeError(format!(
                        "`{relative}` was not part of the verified plugin"
                    )));
                }
                if !digest.matches(&relative, source.as_bytes()) {
                    return Err(mlua::Error::RuntimeError(format!(
                        "`{relative}` changed between verification and loading"
                    )));
                }
            }

            let value: LuaValue = lua
                .load(source)
                .set_name(resolved.display().to_string())
                .set_environment(plugin_env.clone())
                .eval()?;
            let value = if value.is_nil() {
                LuaValue::Boolean(true)
            } else {
                value
            };
            loaded.set(name.as_str(), value.clone())?;
            return Ok(value);
        }

        match &fallback {
            Some(fallback) if !restricted => fallback.call(name),
            _ => Err(mlua::Error::RuntimeError(format!(
                "module `{name}` not found"
            ))),
        }
    })?;

    environment.set("require", require)
}

/// The fixed directory a search template resolves within, canonicalised.
fn template_root(template: &str) -> Option<std::path::PathBuf> {
    use std::path::Path;
    let prefix = template.split('?').next().unwrap_or("");
    let dir = Path::new(prefix).parent().unwrap_or_else(|| Path::new(""));
    std::fs::canonicalize(dir).ok()
}

/// Renders a relative path with `/` separators, matching digest keys.
fn slash_path(path: impl AsRef<std::path::Path>) -> String {
    path.as_ref()
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Which manifests must share a state: connected components of the
/// dependency graph, read as undirected. Returns a component id per name.
fn dependency_components(manifests: &[Manifest]) -> HashMap<String, u64> {
    let mut parent: Vec<u64> = (0..manifests.len()).map(|i| i as u64).collect();

    fn find(parent: &mut [u64], mut node: usize) -> usize {
        while let Some(&up) = parent.get(node) {
            if up as usize == node {
                return node;
            }
            if let Some(&grand) = parent.get(up as usize)
                && let Some(slot) = parent.get_mut(node)
            {
                *slot = grand;
            }
            node = up as usize;
        }
        node
    }

    let position: HashMap<&str, usize> = manifests
        .iter()
        .enumerate()
        .map(|(at, manifest)| (manifest.name.as_str(), at))
        .collect();

    for (at, manifest) in manifests.iter().enumerate() {
        for name in manifest.dependencies.keys() {
            if let Some(&other) = position.get(name.as_str()) {
                let (left, right) = (find(&mut parent, at), find(&mut parent, other));
                if left != right
                    && let Some(slot) = parent.get_mut(left)
                {
                    *slot = right as u64;
                }
            }
        }
    }

    let mut root = HashMap::with_capacity(manifests.len());
    for (at, manifest) in manifests.iter().enumerate() {
        root.insert(manifest.name.clone(), find(&mut parent, at) as u64);
    }
    root
}

impl PluginBackend for LuaBackend {
    fn plugin_type(&self) -> PluginType {
        PluginType::Lua
    }

    fn load(
        &self,
        manifest: &Manifest,
        dir: &std::path::Path,
    ) -> AbiResult<Box<dyn PluginInstance>> {
        let bytes = std::fs::read(manifest.entry_path()).map_err(|e| {
            stanchion_abi::Error::Plugin {
                plugin: manifest.name.clone(),
                reason: format!("reading entry '{}': {e}", manifest.entry),
            }
        })?;
        self.load_bytes(manifest, dir, &bytes)
    }

    fn load_bytes(
        &self,
        manifest: &Manifest,
        _dir: &std::path::Path,
        entry_bytes: &[u8],
    ) -> AbiResult<Box<dyn PluginInstance>> {
        use stanchion_abi::{HostSetup, Rules};
        let setup = HostSetup::default();
        let policy = Rules::deny_all();
        let item = LoadItem {
            manifest,
            signer: stanchion_abi::signature::Signer::Unsigned,
            digest: None,
            entry_bytes: entry_bytes.to_vec(),
        };
        let ctx = LoadContext {
            setup: &setup,
            policy: &policy,
            rock_paths: Vec::new(),
            rocks: None,
            constructor: &self.constructor,
        };
        match self.load_group(std::slice::from_ref(&item), &ctx).pop() {
            Some(GroupOutcome::Loaded { instance, .. }) => Ok(instance),
            Some(GroupOutcome::Failed { reason, .. }) => Err(stanchion_abi::Error::Plugin {
                plugin: manifest.name.clone(),
                reason,
            }),
            None => Err(stanchion_abi::Error::Plugin {
                plugin: manifest.name.clone(),
                reason: "backend returned no outcome".to_string(),
            }),
        }
    }
}

impl Runtime for LuaBackend {
    fn runtime_name(&self) -> &'static str {
        "lua"
    }

    fn load_group(&self, items: &[LoadItem], ctx: &LoadContext) -> Vec<GroupOutcome> {
        // A C rock built for another Lua version would load into a runtime
        // with a different ABI: refuse the whole group loudly rather than
        // each plugin with a confusing link error.
        if let Some(rocks) = ctx.rocks
            && let Err(e) = rocks.check_c_abi(crate::MLUA_LUA_VERSION)
        {
            return items
                .iter()
                .map(|item| GroupOutcome::Failed {
                    name: item.manifest.name.clone(),
                    dir: item.manifest.dir.clone(),
                    reason: e.to_string(),
                })
                .collect();
        }
        let manifests: Vec<&Manifest> = items.iter().map(|item| item.manifest).collect();
        let owned: Vec<Manifest> = manifests.iter().map(|m| (*m).clone()).collect();
        let components = dependency_components(&owned);

        // Per-plugin isolation cannot carry values across states.
        let per_plugin = matches!(self.mode, IsolationMode::PerPlugin(_));

        let mut group_states: HashMap<u64, Arc<SharedState>> = HashMap::new();
        // Exports proxies handed to dependents, by plugin name.
        let mut proxies: HashMap<String, Table> = HashMap::new();
        // Dependency failure short-circuiting, mirroring load order.
        let mut failed: HashSet<String> = HashSet::new();
        let mut outcomes = Vec::with_capacity(items.len());

        // Rock templates are fixed per load, never read from live state.
        let mut rock_templates: Vec<String> = Vec::new();
        for addition in &ctx.rock_paths {
            rock_templates.extend(
                addition
                    .split(';')
                    .filter(|t| !t.is_empty())
                    .map(str::to_string),
            );
        }
        let rock_paths = if ctx.rock_paths.is_empty() {
            None
        } else {
            Some(RockPaths {
                path: ctx.rock_paths.join(";"),
                cpath: None,
            })
        };

        for item in items {
            let outcome = self.load_member(
                item,
                ctx,
                &components,
                per_plugin,
                &mut group_states,
                &mut proxies,
                &failed,
                &rock_templates,
                rock_paths.as_ref(),
            );
            match &outcome {
                GroupOutcome::Loaded { name, .. } => {
                    let _ = name;
                }
                GroupOutcome::Failed { name, .. } => {
                    failed.insert(name.clone());
                }
            }
            outcomes.push(outcome);
        }
        outcomes
    }

    fn reload_plugin(
        &self,
        item: &LoadItem,
        ctx: &LoadContext,
    ) -> AbiResult<Box<dyn PluginInstance>> {
        let name = &item.manifest.name;
        let fail = |reason: String| stanchion_abi::Error::Plugin {
            plugin: name.clone(),
            reason,
        };
        // Reuse the recorded state so dependents keep resolving through
        // their proxies; a missing record builds a fresh state.
        let mut group_states: HashMap<u64, Arc<SharedState>> = HashMap::new();
        let state = self
            .state_for(name, u64::MAX, &mut group_states, ctx.setup, None)
            .map_err(|e| fail(e.to_string()))?;

        // Dependents' live surfaces stay wired; the reloaded plugin keeps
        // seeing the same proxies it was built with.
        let mut dependencies = HashMap::new();
        if let Ok(surfaces) = self.surfaces.lock() {
            for (dep, spec) in &item.manifest.dependencies {
                match surfaces.get(dep) {
                    Some((proxy, _)) => {
                        dependencies.insert(dep.clone(), proxy.clone());
                    }
                    None if spec.is_optional() => {}
                    None => {
                        return Err(fail(format!(
                            "depends on `{dep}`, which did not publish `exports`"
                        )));
                    }
                }
            }
        }
        let mut templates = vec![
            format!("{}/?.lua", item.manifest.dir.display()),
            format!("{}/?/init.lua", item.manifest.dir.display()),
        ];
        templates.extend(ctx.rock_paths.iter().cloned());
        let (instance, _) = self
            .load_one(&state, item, ctx, &templates, dependencies)
            .map_err(fail)?;

        // Repoint the stable proxy at the new surface so every dependent
        // sees the reload without being rebuilt. A plugin that published
        // exports and now does not is a failure, not a silent unpublish.
        let lua = state.lua.lock().map_err(|_| fail("the Lua state is poisoned".to_string()))?;
        let exports = read_exports(&lua, &instance.handle).map_err(fail)?;
        let mut surfaces = self.surfaces.lock().map_err(|_| fail("the Lua state is poisoned".to_string()))?;
        match (surfaces.get(name), exports) {
            (Some((_, metatable)), Some(table)) => {
                metatable.set("__index", table).map_err(|e| fail(e.to_string()))?;
            }
            (None, Some(table)) => {
                let (proxy, metatable) = make_proxy(&lua, &table).map_err(|e| fail(e.to_string()))?;
                surfaces.insert(name.clone(), (proxy, metatable));
            }
            (Some(_), None) => {
                return Err(fail(format!(
                    "`{name}` no longer publishes `exports`; remove and load it again instead"
                )));
            }
            (None, None) => {}
        }
        drop(surfaces);
        drop(lua);
        if let Ok(mut pool) = self.states.lock() {
            pool.insert(name.clone(), state);
        }
        Ok(Box::new(instance))
    }

    fn revoke_capability(
        &self,
        instance: &dyn PluginInstance,
        capability: &str,
    ) -> bool {
        let Some(ours) = instance_to_lua(instance) else {
            return false;
        };
        ours.env.set(capability, LuaValue::Nil).is_ok()
    }

    fn unload(&self, name: &str) {
        if let Ok(mut pool) = self.states.lock() {
            pool.remove(name);
        }
        if let Ok(mut surfaces) = self.surfaces.lock() {
            surfaces.remove(name);
        }
    }
}

fn instance_to_lua(instance: &dyn PluginInstance) -> Option<&LuaInstance> {
    // LuaInstance is the only Lua-backed instance this backend produces.
    instance.as_any().downcast_ref::<LuaInstance>()
}

impl LuaBackend {
    /// Loads one group member, wiring already-loaded dependencies.
    #[allow(clippy::too_many_arguments)]
    fn load_member(
        &self,
        item: &LoadItem,
        ctx: &LoadContext,
        components: &HashMap<String, u64>,
        per_plugin: bool,
        group_states: &mut HashMap<u64, Arc<SharedState>>,
        proxies: &mut HashMap<String, Table>,
        failed: &HashSet<String>,
        rock_templates: &[String],
        rock_paths: Option<&RockPaths>,
    ) -> GroupOutcome {
        let manifest = item.manifest;
        let fail = |reason: String| GroupOutcome::Failed {
            name: manifest.name.clone(),
            dir: manifest.dir.clone(),
            reason,
        };

        if per_plugin
            && let Some((dependency, _)) = manifest.dependencies.first_key_value()
        {
            return fail(format!(
                "depends on `{dependency}`, but per-plugin isolation gives each plugin its own state and values cannot cross states"
            ));
        }
        // A required dependency that failed leaves this plugin unwired.
        if let Some(dep) = manifest
            .dependencies
            .iter()
            .find(|(name, spec)| !spec.is_optional() && failed.contains(*name))
            .map(|(name, _)| name.clone())
        {
            return fail(format!("skipped because `{dep}` failed to load"));
        }

        let group = components.get(&manifest.name).copied().unwrap_or(0);
        let state = match self.state_for(&manifest.name, group, group_states, ctx.setup, rock_paths) {
            Ok(state) => state,
            Err(e) => return fail(format!("creating a plugin state: {e}")),
        };

        // Dependencies resolve to the live proxies of loaded members.
        let mut dependencies = HashMap::new();
        for (name, spec) in &manifest.dependencies {
            match proxies.get(name) {
                Some(proxy) => {
                    dependencies.insert(name.clone(), proxy.clone());
                }
                None if spec.is_optional() => {}
                None => {
                    return fail(format!(
                        "depends on `{name}`, which did not publish `exports`"
                    ));
                }
            }
        }

        // Templates: the plugin's own directory plus rock trees.
        let mut templates = vec![
            format!("{}/?.lua", manifest.dir.display()),
            format!("{}/?/init.lua", manifest.dir.display()),
        ];
        templates.extend(rock_templates.iter().cloned());

        // A panic while evaluating the chunk or running the constructor
        // is this plugin's failure rather than the whole load's.
        let built = stanchion_abi::panics::guard(|| {
            self.load_one(&state, item, ctx, &templates, dependencies)
        });
        let (instance, granted) = match built {
            Ok(Ok(loaded)) => loaded,
            Ok(Err(reason)) => return fail(reason),
            Err(panicked) => return fail(panicked.to_string()),
        };

        // Publish exports behind a stable proxy dependents hold.
        let lua = match state.lua.lock() {
            Ok(lua) => lua,
            Err(_) => return fail("the Lua state is poisoned".to_string()),
        };
        let exports = match read_exports(&lua, &instance.handle) {
            Ok(exports) => exports,
            Err(reason) => return fail(reason),
        };
        if let Some(table) = &exports {
            match make_proxy(&lua, table) {
                Ok((proxy, metatable)) => {
                    proxies.insert(manifest.name.clone(), proxy.clone());
                    if let Ok(mut surfaces) = self.surfaces.lock() {
                        surfaces.insert(manifest.name.clone(), (proxy, metatable));
                    }
                }
                Err(e) => return fail(e.to_string()),
            }
        }
        drop(lua);

        GroupOutcome::Loaded {
            name: manifest.name.clone(),
            instance: Box::new(instance),
            granted,
        }
    }
}

/// Reads a plugin's `exports`: a table, or a function returning one.
fn read_exports(_lua: &Lua, handle: &crate::LuaHandle) -> std::result::Result<Option<Table>, String> {
    let value: LuaValue = handle.get(EXPORTS_KEY).map_err(|e| e.to_string())?;
    match value {
        LuaValue::Nil => Ok(None),
        LuaValue::Table(table) => Ok(Some(table)),
        LuaValue::Function(function) => {
            let table: Table = function
                .call(handle.to_value())
                .map_err(|e: mlua::Error| e.to_string())?;
            Ok(Some(table))
        }
        other => Err(format!(
            "`{EXPORTS_KEY}` must be a table or a function returning one, found {}",
            other.type_name()
        )),
    }
}

/// Wraps an exports table in the stable proxy dependents hold, returning the
/// proxy plus its metatable so reload can repoint dependents at a new table.
///
/// Only reads forward; writes are refused and the metatable is locked so a
/// dependent cannot repoint the surface for everyone else.
fn make_proxy(lua: &Lua, table: &Table) -> mlua::Result<(Table, Table)> {
    let proxy = lua.create_table()?;
    let metatable = lua.create_table()?;
    metatable.set("__index", table.clone())?;
    let readonly = lua.create_function(|_, (_, _, _): (Table, LuaValue, LuaValue)| {
        Err::<(), _>(mlua::Error::RuntimeError(
            "plugin exports are read-only".to_string(),
        ))
    })?;
    metatable.set("__newindex", readonly)?;
    metatable.set("__metatable", "locked: plugin exports")?;
    proxy.set_metatable(Some(metatable.clone()))?;
    Ok((proxy, metatable))
}

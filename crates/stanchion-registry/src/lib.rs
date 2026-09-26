//! A registry of Lua plugins, in one shared Lua state or one state per plugin.
//!
//! Plugins live in `<root>/<plugin>/plugin.toml` alongside their entry chunk. Each is
//! evaluated as a [`LuaClass`] and constructed once, with its manifest's `[config]`
//! table passed to the constructor:
//!
//! ```lua
//! local Greeter = {}
//! Greeter.__index = Greeter
//!
//! function Greeter.new(config)
//!   return setmetatable({ greeting = config.greeting }, Greeter)
//! end
//! ```
//!
//! # Isolation
//!
//! All plugins share one [`Lua`], so they share `require`d modules and host functions.
//! Each chunk is evaluated with its own environment table whose `__index` is the real
//! globals: a plugin **reads** globals normally but its **writes** stay local, so one
//! plugin cannot redefine `string.format` for the others. Resource limits are a
//! property of the whole state, not of a plugin — if you need per-plugin memory or
//! instruction limits, you need one [`Lua`] per plugin instead.

mod capability;
#[cfg(feature = "config")]
pub mod config;
pub mod dynamic;
mod error;
#[cfg(feature = "signatures")]
pub mod lock;
mod manifest;
mod panics;
mod capability;
#[cfg(feature = "config")]
pub mod config;
pub mod dynamic;
mod error;
#[cfg(feature = "signatures")]
pub mod lock;
mod manifest;
mod panics;
mod runtime;
#[cfg(feature = "signatures")]
pub mod signature;
pub mod upgrade;

pub use dynamic::{DynClass, DynInstance};
pub use error::{FailureReason, LoadFailure, RegistryError};
pub use stanchion_abi::runtime::Runtime;
pub use stanchion_abi::manifest::{
    DependencySpec, DetailedDependency, MANIFEST_FILE, Manifest, PluginType,
};
pub use manifest::{discover, read_manifest, resolve_order};
pub use panics::Panicked;
/// Re-exported because [`Decision::GrantWith`] takes a `toml::Table`: a public API
/// that names a foreign type has to hand you that type.
pub use toml;

pub use capability::{CapabilityRequest, Decision, Grant, HostSetup, OPTIONAL_KEY, Policy, Rules};
#[cfg(feature = "config")]
pub use config::{CapabilityConfig, HostConfig, SandboxConfig, SignatureConfig, load_config};
#[cfg(feature = "signatures")]
pub use lock::{LOCK_FILE, LockError, LockedPlugin, Lockfile};
pub use upgrade::{Change, UpgradeReview};
#[cfg(feature = "signatures")]
pub use signature::{
    BUNDLE_FILE, DirectoryDigest, PluginVerifier, Revocation, Revocations, SIGNATURE_FILE, Signer,
    VerifyError,
};

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::Path;

use mlua::{Lua, LuaSerdeExt, Table, Value};

use stanchion_lua::{Budget, LuaClass, LuaObject, Sandbox};

/// Constructor looked up on a plugin's class table when none is configured.
pub const DEFAULT_CONSTRUCTOR: &str = "new";

/// Which plugins have to share a Lua state.
///
/// Two plugins must share one exactly when a chain of `[dependencies]` connects them,
/// because that chain is what carries Lua values between them. So the grouping is the
/// connected components of the dependency graph, read as undirected: a dependency binds
/// both ends.
///
/// Returns a component representative per plugin name. A dependency has to be present
/// in the same load for the plugin to load at all, so a component is always resolved
/// within one call and never has to join a state that is already running.
fn dependency_components(manifests: &[Manifest]) -> HashMap<String, usize> {
    // Union-find over manifest positions, with path halving. Plugin counts are small,
    // so this is written to be obviously correct rather than to be fast.
    let mut parent: Vec<usize> = (0..manifests.len()).collect();

    fn find(parent: &mut [usize], mut node: usize) -> usize {
        while let Some(&up) = parent.get(node) {
            if up == node {
                return node;
            }
            // Halve the path on the way up so repeated lookups stay cheap.
            if let Some(&grand) = parent.get(up)
                && let Some(slot) = parent.get_mut(node)
            {
                *slot = grand;
            }
            node = up;
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
                    *slot = right;
                }
            }
        }
    }

    // A representative only means anything once every union is done.
    let mut root = HashMap::with_capacity(manifests.len());
    for (at, manifest) in manifests.iter().enumerate() {
        let representative = find(&mut parent, at);
        root.insert(manifest.name.clone(), representative);
    }
    root
}

/// Key a plugin publishes its public surface under.
pub const EXPORTS_KEY: &str = "exports";

/// A plugin's published surface, behind a handle that survives reload.
///
/// Dependents receive `proxy`, an empty table whose metatable forwards reads and
/// writes to the live exports table. Reloading the provider repoints the metatable,
/// so every dependent sees the new surface without being rebuilt.
#[derive(Clone)]
struct Exports {
    proxy: Table,
    metatable: Table,
}

impl Exports {
    /// Points the stable proxy at a freshly built exports table.
    fn repoint(&self, table: Table) -> mlua::Result<()> {
        self.metatable.set("__index", table.clone())?;
        self.metatable.set("__newindex", table)?;
        Ok(())
    }
}

/// A loaded plugin: its manifest and its constructed instance.
pub struct Plugin<C: LuaClass> {
    lua: Lua,
    budget: Option<Budget>,
    environment: Table,
    granted: Vec<String>,
    group: Group,
    #[cfg(feature = "signatures")]
    signer: Signer,
    // Kept so a revocation list arriving after load can be applied without going back
    // to the filesystem, where the bytes may no longer be the ones that were verified.
    #[cfg(feature = "signatures")]
    digest: Option<DirectoryDigest>,
    manifest: Manifest,
    instance: C::Instance,
    exports: Option<Exports>,
}

impl<C: LuaClass> Plugin<C> {
    /// The plugin's manifest, including its `[config]` table and directory.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The plugin's name, as declared in its manifest.
    pub fn name(&self) -> &str {
        &self.manifest.name
    }

    /// The constructed instance, which implements the `#[lua_class]` trait.
    pub fn instance(&self) -> &C::Instance {
        &self.instance
    }

    /// The Lua state this plugin runs in.
    ///
    /// Under [`Isolation::Shared`] every plugin returns the same state; under
    /// per-plugin isolation each returns its own.
    pub fn lua(&self) -> &Lua {
        &self.lua
    }

    /// This plugin's instruction allowance, when one is configured.
    ///
    /// Under [`Isolation::PerGroup`] the allowance belongs to the group, so every
    /// member of a dependency chain reports the same one.
    pub fn budget(&self) -> Option<&Budget> {
        self.budget.as_ref()
    }

    /// Which state this plugin runs in.
    ///
    /// Meaningful under [`Isolation::PerGroup`], where two plugins reporting the same
    /// [`Group`] share a state; the other modes report group `0` for everything.
    pub fn group(&self) -> Group {
        self.group
    }

    /// Who signed this plugin.
    #[cfg(feature = "signatures")]
    pub fn signer(&self) -> &Signer {
        &self.signer
    }

    /// The digest of the bytes this plugin was loaded from, when one was computed.
    ///
    /// `None` when nothing needed it: no verifier, no lockfile and no revocation list
    /// were configured, so the directory was never hashed.
    #[cfg(feature = "signatures")]
    pub fn digest(&self) -> Option<&DirectoryDigest> {
        self.digest.as_ref()
    }

    /// Capabilities this plugin was actually granted, after policy ran.
    pub fn granted_capabilities(&self) -> impl Iterator<Item = &str> {
        self.granted.iter().map(String::as_str)
    }

    /// The environment granted capabilities are bound in.
    pub fn environment(&self) -> &Table {
        &self.environment
    }

    /// The stable handle dependents receive, or `None` if this plugin publishes nothing.
    ///
    /// Reads and writes forward to the plugin's current exports table, so the handle
    /// stays valid across reloads of this plugin.
    pub fn exports(&self) -> Option<&Table> {
        self.exports.as_ref().map(|exports| &exports.proxy)
    }
}

/// What one `load_dir` call did.
#[derive(Debug, Default)]
pub struct LoadReport {
    /// Names of plugins that loaded, in the order they were constructed.
    pub loaded: Vec<String>,
    /// Plugins that did not load. Loading never stops at the first failure.
    pub failures: Vec<LoadFailure>,
}

impl LoadReport {
    /// True when every discovered plugin loaded.
    pub fn is_clean(&self) -> bool {
        self.failures.is_empty()
    }
}

/// What one [`Registry::install_rocks`] call did.
#[cfg(feature = "luarocks")]
#[derive(Debug, Default)]
pub struct InstallReport {
    /// Rocks fetched into the tree.
    pub installed: Vec<String>,
    /// Rocks already present at an acceptable version.
    pub satisfied: Vec<String>,
    /// Rocks that could not be installed, with the reason.
    pub failures: Vec<(String, String)>,
}

/// What every plugin under a root asks for, read from manifests alone.
#[derive(Debug)]
pub struct Audit {
    /// One entry per discovered plugin.
    pub plugins: Vec<PluginAudit>,
    /// Labels of values installed ambiently, which reach every plugin.
    pub ambient: Vec<String>,
    /// Capability names the host is able to provide.
    pub offered: Vec<String>,
    /// Plugin directories whose manifest could not be read.
    pub unreadable: Vec<LoadFailure>,
}

impl Audit {
    /// Requests naming a capability the host does not offer.
    pub fn unsatisfiable(&self) -> impl Iterator<Item = &CapabilityRequest> {
        self.plugins
            .iter()
            .flat_map(|plugin| plugin.requests.iter())
            .filter(|request| !self.offered.contains(&request.name))
    }
}

/// One plugin's declared requests.
#[derive(Debug)]
pub struct PluginAudit {
    /// Plugin name.
    pub name: String,
    /// Where it was discovered.
    pub dir: std::path::PathBuf,
    /// Capabilities it declares.
    pub requests: Vec<CapabilityRequest>,
    /// Who signed it, established without running any of its code.
    #[cfg(feature = "signatures")]
    pub signer: Signer,
}

/// Everything `instantiate` produces for one plugin.
struct Loaded<C: LuaClass> {
    instance: C::Instance,
    exports: Option<Table>,
    environment: Table,
    granted: Vec<String>,
}

/// One plugin's result from a dispatch.
#[derive(Debug)]
pub struct Outcome<'a, R> {
    /// The plugin that produced this result.
    pub name: &'a str,
    /// What the call returned. A failure here does not affect the other plugins.
    pub result: mlua::Result<R>,
}

/// How plugin states relate to each other.
pub enum Isolation {
    /// Every plugin runs in the state handed to [`Registry::new`].
    Shared,
    /// Every plugin gets its own state, built under a [`Sandbox`] policy.
    ///
    /// Memory and instruction limits are properties of a Lua state, so this is the
    /// only mode in which they can be enforced per plugin. The cost is that Lua
    /// values cannot cross states, so `[dependencies]` exports cannot be injected.
    PerPlugin(Box<Sandbox>),
    /// One state per dependency group, built under a [`Sandbox`] policy.
    ///
    /// Plugins wired together by `[dependencies]` share a state, because that is what
    /// lets exports cross between them; plugins with nothing between them are kept
    /// apart. The group, not the plugin, is then the accounting unit: memory and
    /// instruction limits apply to the whole group.
    PerGroup(Box<Sandbox>),
}

/// Which state a plugin runs in, when the registry is the one deciding.
///
/// Two plugins reporting the same group share a Lua state, and so share a heap, a
/// memory limit and an instruction budget. Under the other isolation modes every plugin
/// reports group `0`, which is the truth: one state for all of them, or one each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Group(u64);

impl fmt::Display for Group {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A policy decision, boxed for storage.
#[cfg(feature = "send")]
type PolicyFn = Box<dyn Policy + Send + Sync>;

/// A policy decision, boxed for storage.
#[cfg(not(feature = "send"))]
type PolicyFn = Box<dyn Policy>;

/// Plugins of one class.
pub struct Registry<C: LuaClass> {
    host: Lua,
    isolation: Isolation,
    host_setup: HostSetup,
    setup_error: Option<mlua::Error>,
    shared_configured: bool,
    policy: Option<PolicyFn>,
    constructor: String,
    plugins: Vec<Plugin<C>>,
    index: HashMap<String, usize>,
    next_group: u64,
    #[cfg(feature = "signatures")]
    verifier: Option<Box<dyn PluginVerifier>>,
    #[cfg(feature = "signatures")]
    require_signatures: bool,
    #[cfg(feature = "signatures")]
    revocations: Option<signature::Revocations>,
    #[cfg(feature = "signatures")]
    lockfile: Option<lock::Lockfile>,
    #[cfg(feature = "luarocks")]
    rocks: Option<rocks::RocksConfig>,
    #[cfg(feature = "luarocks")]
    rock_paths: Option<rocks::RockPaths>,
}

impl<C: LuaClass> Registry<C> {
    /// Creates an empty registry over an existing Lua state.
    ///
    /// Register host functions on the state before loading, since plugin chunks read
    /// globals through their environment's `__index`.
    pub fn new(lua: Lua) -> Self {
        Registry {
            host: lua,
            isolation: Isolation::Shared,
            host_setup: HostSetup::default(),
            setup_error: None,
            shared_configured: false,
            policy: None,
            constructor: DEFAULT_CONSTRUCTOR.to_string(),
            plugins: Vec::new(),
            index: HashMap::new(),
            next_group: 0,
            #[cfg(feature = "signatures")]
            verifier: None,
            #[cfg(feature = "signatures")]
            require_signatures: false,
            #[cfg(feature = "signatures")]
            revocations: None,
            #[cfg(feature = "signatures")]
            lockfile: None,
            #[cfg(feature = "luarocks")]
            rocks: None,
            #[cfg(feature = "luarocks")]
            rock_paths: None,
        }
    }

    /// Gives every plugin its own Lua state, built under `sandbox`.
    ///
    /// This is what makes per-plugin memory and instruction limits possible, since
    /// both are properties of a state rather than of a table or function. In exchange,
    /// plugins cannot exchange Lua values, so a `[dependencies]` entry that would
    /// inject exports fails to load.
    ///
    /// `host` is the state the registry itself keeps; plugins never see it.
    pub fn isolated(host: Lua, sandbox: Sandbox) -> Self {
        let mut registry = Registry::new(host);
        registry.isolation = Isolation::PerPlugin(Box::new(sandbox));
        registry
    }

    /// Gives every *dependency group* its own Lua state, built under `sandbox`.
    ///
    /// This is [`Registry::isolated`] without the trade-off that `[dependencies]` stops
    /// working. A Lua value still cannot cross states, so the registry puts the plugins
    /// that need to exchange values in the same one: each connected component of the
    /// dependency graph gets a state, and a plugin depending on nothing gets one to
    /// itself.
    ///
    /// What that buys is bounded rather than free. Group members share a heap, a memory
    /// limit and an instruction budget, and they see each other's globals — declaring a
    /// dependency is declaring that you accept that. Plugins in different groups are as
    /// separated as they are under per-plugin isolation.
    ///
    /// `host` is the state the registry itself keeps; plugins never see it.
    pub fn grouped(host: Lua, sandbox: Sandbox) -> Self {
        let mut registry = Registry::new(host);
        registry.isolation = Isolation::PerGroup(Box::new(sandbox));
        registry
    }

    /// Declares everything plugins can reach.
    ///
    /// Runs once, before the first plugin loads. Capabilities registered here are
    /// gated: a plugin gets one only by declaring it and passing [`Policy`]. Anything
    /// registered with [`HostSetup::ambient`] is ungated and reaches every plugin.
    ///
    /// ```ignore
    /// Registry::isolated(Lua::new(), Sandbox::restricted())
    ///     .with_setup(|host| {
    ///         host.capability("log", |lua, _grant| {
    ///             Ok(Value::Function(lua.create_function(|_, m: String| {
    ///                 println!("{m}");
    ///                 Ok(())
    ///             })?))
    ///         });
    ///         host.ambient("HOST_VERSION", |lua| lua.globals().set("HOST_VERSION", "1.0"));
    ///         Ok(())
    ///     })
    ///     .with_policy(Rules::deny_all().allow("log"))
    /// ```
    pub fn with_setup(mut self, setup: impl FnOnce(&mut HostSetup) -> mlua::Result<()>) -> Self {
        let mut host_setup = HostSetup::default();
        // Collected immediately so `audit` can report the host's offer before any
        // plugin loads. A failure is held and surfaced by the next fallible call.
        match setup(&mut host_setup) {
            Ok(()) => self.host_setup = host_setup,
            Err(err) => self.setup_error = Some(err),
        }
        self
    }

    /// Decides which requested capabilities are actually granted.
    ///
    /// Without a policy every capability is denied, even one with a registered
    /// provider: offering a capability and granting it are separate decisions.
    pub fn with_policy(mut self, policy: impl Policy + 'static) -> Self {
        self.policy = Some(Box::new(policy));
        self
    }

    /// What the host offers, once setup has run.
    pub fn host_setup(&self) -> &HostSetup {
        &self.host_setup
    }

    /// Checks every plugin's signature before it loads.
    ///
    /// Verification covers every file in the plugin directory, and the loader
    /// re-checks each file's hash as it reads it, so the bytes that run are the bytes
    /// that were verified.
    #[cfg(feature = "signatures")]
    pub fn with_verifier(mut self, verifier: impl PluginVerifier + 'static) -> Self {
        self.verifier = Some(Box::new(verifier));
        self
    }

    /// Whether an unsigned plugin is refused outright.
    ///
    /// When `false` (the default) an unsigned plugin loads as [`Signer::Unsigned`],
    /// and capability policy can still refuse it privileges — signing becomes a
    /// gradient rather than a cliff. When `true` every plugin must be signed.
    #[cfg(feature = "signatures")]
    pub fn require_signatures(mut self, required: bool) -> Self {
        self.require_signatures = required;
        self
    }

    /// Refuses builds or signers on a revocation list.
    ///
    /// Checked after verification, because a withdrawn plugin's signature is still
    /// valid — that is precisely why a separate, mutable list is needed. Works without
    /// a verifier too: a digest denylist refuses a specific build with no signing
    /// infrastructure at all.
    #[cfg(feature = "signatures")]
    pub fn with_revocations(mut self, revocations: signature::Revocations) -> Self {
        self.revocations = Some(revocations);
        self
    }

    /// Refuses any plugin whose bytes are not the ones this lockfile pins.
    ///
    /// This is the control that makes a transport untrusted: a package source, mirror
    /// or index can serve whatever it likes, and anything other than the pinned digest
    /// fails to load. It needs no signing infrastructure — a lockfile alone already
    /// refuses substitution and downgrade, neither of which a signature stops.
    ///
    /// Every discovered plugin must be pinned. One in the root with no entry fails as
    /// [`LockError::Unlocked`] rather than loading, because once a host keeps a
    /// lockfile, an unpinned directory appearing beside the pinned ones is exactly the
    /// event worth refusing.
    #[cfg(feature = "signatures")]
    pub fn with_lockfile(mut self, lockfile: lock::Lockfile) -> Self {
        self.lockfile = Some(lockfile);
        self
    }

    /// The lockfile this registry enforces, if any.
    #[cfg(feature = "signatures")]
    pub fn lockfile(&self) -> Option<&lock::Lockfile> {
        self.lockfile.as_ref()
    }

    /// How plugin states are allocated.
    pub fn isolation(&self) -> &Isolation {
        &self.isolation
    }

    /// Verifies `[rocks]` declarations against a LuaRocks tree and puts that tree on
    /// the shared state's module path.
    ///
    /// Without this, a plugin declaring `[rocks]` fails to load rather than silently
    /// resolving its `require`s from whatever happens to be on the machine.
    #[cfg(feature = "luarocks")]
    pub fn with_rocks(mut self, config: rocks::RocksConfig) -> Self {
        self.rocks = Some(config);
        self
    }

    /// Uses a different constructor name than `new`.
    pub fn with_constructor(mut self, name: impl Into<String>) -> Self {
        self.constructor = name.into();
        self
    }

    /// The state the registry holds.
    ///
    /// Under [`Isolation::Shared`] this is the state every plugin runs in. Under
    /// per-plugin isolation it is the host's own state, which no plugin can see; use
    /// [`Plugin::lua`] to reach a particular plugin's state.
    pub fn lua(&self) -> &Lua {
        &self.host
    }

    /// Produces a state to run plugins in, applying setup and rock paths.
    ///
    /// Under [`Isolation::PerGroup`] this makes a state for *one* group; `load_dir`
    /// decides which plugins share it.
    fn acquire_state(&mut self) -> Result<(Lua, Option<Budget>), RegistryError> {
        if let Isolation::PerPlugin(sandbox) | Isolation::PerGroup(sandbox) = &self.isolation {
            let sandbox = sandbox.clone();
            let (lua, budget) = sandbox.build().map_err(RegistryError::Lua)?;
            let runtime = crate::runtime::LuaRuntime::new(lua.clone());
            self.configure(&runtime)?;
            return Ok((lua, budget));
        }

        let lua = self.host.clone();
        let runtime = crate::runtime::LuaRuntime::new(lua.clone());
        if !self.shared_configured {
            self.configure(&runtime)?;
            self.shared_configured = true;
        }
        Ok((lua, None))
    }

    /// Decides which state one plugin runs in, creating it on the group's first member.
    fn group_state(
        &mut self,
        components: Option<&HashMap<String, usize>>,
        states: &mut HashMap<usize, (Lua, Option<Budget>, Group)>,
        manifest: &Manifest,
    ) -> Result<(Lua, Option<Budget>, Group), RegistryError> {
        // Not grouping, or a manifest that somehow was not partitioned: either way the
        // plugin gets whatever the isolation mode hands out, on its own.
        let Some(representative) = components.and_then(|root| root.get(&manifest.name)).copied()
        else {
            let (lua, budget) = self.acquire_state()?;
            return Ok((lua, budget, Group(0)));
        };

        if let Some((lua, budget, group)) = states.get(&representative) {
            return Ok((lua.clone(), budget.clone(), *group));
        }

        let (lua, budget) = self.acquire_state()?;
        let state = (lua, budget, self.fresh_group());
        states.insert(representative, state.clone());
        Ok(state)
    }

    /// Hands out the next group identifier.
    fn fresh_group(&mut self) -> Group {
        let group = Group(self.next_group);
        self.next_group = self.next_group.saturating_add(1);
        group
    }

    /// Surfaces a failure from the `with_setup` closure, which ran at build time.
    fn check_setup(&self) -> Result<(), RegistryError> {
        match &self.setup_error {
            Some(err) => Err(RegistryError::Lua(err.clone())),
            None => Ok(()),
        }
    }

    /// Installs ambient globals and cached rock paths on one state.
    fn configure(&self, runtime: &dyn Runtime) -> Result<(), RegistryError> {
        self.host_setup
            .install_ambient(runtime)
            .map_err(RegistryError::Lua)?;
        #[cfg(feature = "luarocks")]
        if let Some(paths) = &self.rock_paths {
            // Need a Lua state to prepend paths
            // This will be a separate issue
            unimplemented!("LuaRuntime::configure - luarocks paths")
        }
        Ok(())
    }

    /// Discovers, orders and loads every plugin under `root`.
    ///
    /// Only an unreadable root is fatal. A plugin with a broken manifest, a failing
    /// chunk, a missing dependency, or a place in a dependency cycle is reported in
    /// [`LoadReport::failures`] while the rest still load.
    pub fn load_dir(&mut self, root: impl AsRef<Path>) -> Result<LoadReport, RegistryError> {
        self.check_setup()?;
        let (manifests, mut failures) = manifest::discover(root.as_ref())?;
        let (ordered, order_failures) = manifest::resolve_order(manifests);
        failures.extend(order_failures);

        #[cfg(feature = "luarocks")]
        let installed_rocks = self.prepare_rocks()?;

        // Under grouped isolation the partition is decided before anything is built, so
        // a plugin's state is a property of the whole dependency graph rather than of
        // whichever member happened to load first.
        let components = if matches!(self.isolation, Isolation::PerGroup(_)) {
            Some(dependency_components(&ordered))
        } else {
            None
        };
        // The state each component runs in, made on its first member.
        let mut group_states: HashMap<usize, (Lua, Option<Budget>, Group)> = HashMap::new();

        let mut failed: HashSet<String> = failures.iter().map(|f| f.name.clone()).collect();
        let mut loaded = Vec::new();

        for manifest in ordered {
            if self.index.contains_key(&manifest.name) {
                failed.insert(manifest.name.clone());
                failures.push(LoadFailure {
                    name: manifest.name.clone(),
                    dir: manifest.dir.clone(),
                    reason: FailureReason::Manifest(format!(
                        "another plugin is already registered as `{}`",
                        manifest.name
                    )),
                });
                continue;
            }

            // A required dependency that failed leaves this plugin unwired; an
            // optional one just stays nil.
            let unmet = manifest
                .dependencies
                .iter()
                .find(|(name, spec)| !spec.is_optional() && failed.contains(*name))
                .map(|(name, _)| name.clone());
            if let Some(dep) = unmet {
                failed.insert(manifest.name.clone());
                failures.push(LoadFailure {
                    name: manifest.name.clone(),
                    dir: manifest.dir.clone(),
                    reason: FailureReason::DependencyFailed(dep),
                });
                continue;
            }

            if let Isolation::PerPlugin(_) = self.isolation
                && let Some((dependency, _)) = manifest.dependencies.first_key_value()
            {
                failed.insert(manifest.name.clone());
                failures.push(LoadFailure {
                    name: manifest.name.clone(),
                    dir: manifest.dir.clone(),
                    reason: FailureReason::CrossStateDependency(dependency.clone()),
                });
                continue;
            }

            #[cfg(feature = "luarocks")]
            if let Err(reason) = verify_rocks(&manifest, installed_rocks.as_ref()) {
                failed.insert(manifest.name.clone());
                failures.push(LoadFailure {
                    name: manifest.name.clone(),
                    dir: manifest.dir.clone(),
                    reason,
                });
                continue;
            }
            #[cfg(not(feature = "luarocks"))]
            if !manifest.rocks.is_empty() {
                failed.insert(manifest.name.clone());
                failures.push(LoadFailure {
                    name: manifest.name.clone(),
                    dir: manifest.dir.clone(),
                    reason: FailureReason::Rocks(
                        "declares `[rocks]` but the `luarocks` feature is not enabled".to_string(),
                    ),
                });
                continue;
            }

            #[cfg(feature = "signatures")]
            let (signer, digest) = match self.verify_plugin(&manifest) {
                Ok(verified) => verified,
                Err(reason) => {
                    failed.insert(manifest.name.clone());
                    failures.push(LoadFailure {
                        name: manifest.name.clone(),
                        dir: manifest.dir.clone(),
                        reason,
                    });
                    continue;
                }
            };

            let (lua, budget, group) =
                self.group_state(components.as_ref(), &mut group_states, &manifest)?;
            let runtime = crate::runtime::LuaRuntime::new(lua.clone());
            // Evaluating a chunk and running a constructor is plugin code, so a panic
            // there is this plugin's failure rather than the whole load's.
            let built = panics::guard(|| {
                self.instantiate(
                    &lua,
                    &runtime,
                    budget.as_ref(),
                    &manifest,
                    #[cfg(feature = "signatures")]
                    &signer,
                    #[cfg(feature = "signatures")]
                    digest.as_ref(),
                )
            })
            .unwrap_or_else(|panicked| Err(FailureReason::Panicked(panicked)));
            match built.and_then(|built| {
                let exports = built
                    .exports
                    .map(|table| self.make_proxy(&lua, &table))
                    .transpose()?;
                Ok((built.instance, exports, built.environment, built.granted))
            }) {
                Ok((instance, exports, environment, granted)) => {
                    loaded.push(manifest.name.clone());
                    self.index.insert(manifest.name.clone(), self.plugins.len());
                    self.plugins.push(Plugin {
                        lua,
                        budget,
                        environment,
                        granted,
                        group,
                        #[cfg(feature = "signatures")]
                        signer,
                        #[cfg(feature = "signatures")]
                        digest,
                        manifest,
                        instance,
                        exports,
                    });
                }
                Err(reason) => {
                    failed.insert(manifest.name.clone());
                    failures.push(LoadFailure {
                        name: manifest.name.clone(),
                        dir: manifest.dir.clone(),
                        reason,
                    });
                }
            }
        }

        Ok(LoadReport { loaded, failures })
    }

    /// Re-reads a plugin's manifest and chunk and swaps in a fresh instance.
    ///
    /// The old instance is dropped only once the caller releases it, so handles held
    /// across a reload stay valid and keep talking to the old object.
    pub fn reload(&mut self, name: &str) -> Result<(), RegistryError> {
        let position = *self
            .index
            .get(name)
            .ok_or_else(|| RegistryError::UnknownPlugin(name.to_string()))?;
        let current = self
            .plugins
            .get(position)
            .ok_or_else(|| RegistryError::UnknownPlugin(name.to_string()))?;
        let dir = current.manifest.dir.clone();
        let existing = current.exports.clone();
        let lua = current.lua.clone();
        let budget = current.budget.clone();
        let group = current.group;

        let fail = |reason: FailureReason| {
            RegistryError::Reload(Box::new(LoadFailure {
                name: name.to_string(),
                dir: dir.clone(),
                reason,
            }))
        };

        let manifest = manifest::read_manifest(&dir).map_err(&fail)?;
        if manifest.name != name {
            return Err(fail(FailureReason::Manifest(format!(
                "manifest renamed the plugin to `{}`; remove and load it again instead",
                manifest.name
            ))));
        }

        #[cfg(feature = "signatures")]
        let (signer, digest) = self.verify_plugin(&manifest).map_err(&fail)?;

        let runtime = crate::runtime::LuaRuntime::new(lua.clone());
        let built = panics::guard(|| {
                self.instantiate(
                    &lua,
                    &runtime,
                    budget.as_ref(),
                    &manifest,
                    #[cfg(feature = "signatures")]
                    &signer,
                    #[cfg(feature = "signatures")]
                    digest.as_ref(),
                )
        })
        .unwrap_or_else(|panicked| Err(FailureReason::Panicked(panicked)))
        .map_err(&fail)?;
        let (instance, table) = (built.instance, built.exports);

        // Reusing the old proxy is what makes a reload visible to dependents: they
        // hold that table, and repointing it swaps the surface underneath them.
        let exports = match (existing, table) {
            (Some(exports), Some(table)) => {
                exports.repoint(table).map_err(|err| fail(err.into()))?;
                Some(exports)
            }
            (None, Some(table)) => Some(
                self.make_proxy(&lua, &table)
                    .map_err(|err| fail(err.into()))?,
            ),
            (Some(_), None) => {
                return Err(fail(FailureReason::MissingExports(name.to_string())));
            }
            (None, None) => None,
        };

        let slot = self
            .plugins
            .get_mut(position)
            .ok_or_else(|| RegistryError::UnknownPlugin(name.to_string()))?;
        *slot = Plugin {
            lua,
            budget,
            environment: built.environment,
            granted: built.granted,
            group,
            #[cfg(feature = "signatures")]
            signer,
            #[cfg(feature = "signatures")]
            digest,
            manifest,
            instance,
            exports,
        };
        Ok(())
    }

    /// Unloads one plugin, returning it so the caller decides when it is dropped.
    ///
    /// Dropping it releases the plugin's Lua state under per-plugin isolation, and its
    /// exports proxy stops resolving, so dependents see the table empty rather than
    /// stale. Handles the caller still holds keep working until then.
    pub fn remove(&mut self, name: &str) -> Result<Plugin<C>, RegistryError> {
        let position = *self
            .index
            .get(name)
            .ok_or_else(|| RegistryError::UnknownPlugin(name.to_string()))?;
        if position >= self.plugins.len() {
            return Err(RegistryError::UnknownPlugin(name.to_string()));
        }
        let plugin = self.plugins.remove(position);

        // Removal shifts every later plugin down, so the name index is rebuilt rather
        // than patched: an off-by-one here would hand a caller another plugin.
        self.index.clear();
        for (position, plugin) in self.plugins.iter().enumerate() {
            self.index.insert(plugin.manifest.name.clone(), position);
        }
        Ok(plugin)
    }

    /// Replaces the revocation list and unloads every loaded plugin it now names.
    ///
    /// A revocation list is the mutable half of provenance: it changes while your
    /// process is running, which is precisely when it matters. Consulting it only at
    /// load would mean a plugin withdrawn at noon keeps running until something else
    /// happens to reload it. This applies a new list to what is already loaded, and
    /// to every later load.
    ///
    /// Each returned [`LoadFailure`] names a plugin that was unloaded and why. The
    /// plugins themselves are dropped here, so a caller holding an instance across
    /// this call keeps talking to a plugin the host has just refused — take the
    /// unloaded names as the signal to release those handles.
    ///
    /// A digest-bearing list needs a digest to compare against. Plugins loaded with
    /// one recorded are checked against that, not against the directory as it stands
    /// now. For a plugin loaded without one — nothing at load needed it — the
    /// directory is hashed here; if that read fails the plugin is unloaded with the
    /// I/O error as its reason, because a trust decision that cannot be made is not
    /// one to resolve in the plugin's favour.
    #[cfg(feature = "signatures")]
    pub fn apply_revocations(&mut self, revocations: Revocations) -> Vec<LoadFailure> {
        let mut refused: Vec<LoadFailure> = Vec::new();

        if !revocations.is_empty() {
            let needs_digest = revocations.needs_digest();
            for plugin in &self.plugins {
                let digest = match (&plugin.digest, needs_digest) {
                    (Some(digest), _) => Some(::std::borrow::Cow::Borrowed(digest)),
                    (None, false) => None,
                    (None, true) => match DirectoryDigest::compute(&plugin.manifest.dir) {
                        Ok(digest) => Some(::std::borrow::Cow::Owned(digest)),
                        Err(source) => {
                            refused.push(LoadFailure {
                                name: plugin.manifest.name.clone(),
                                dir: plugin.manifest.dir.clone(),
                                reason: FailureReason::Io(source),
                            });
                            continue;
                        }
                    },
                };
                if let Some(reason) = revocations.check(digest.as_deref(), &plugin.signer) {
                    refused.push(LoadFailure {
                        name: plugin.manifest.name.clone(),
                        dir: plugin.manifest.dir.clone(),
                        reason: FailureReason::Revoked(reason),
                    });
                }
            }
        }

        // Stored before the removals so a later load is judged by the same list, and
        // after the loop so the loop reads the list it was handed.
        self.revocations = Some(revocations);

        for failure in &refused {
            // The name came from the plugin list a moment ago, so a failure to find it
            // is not something a caller can act on: the unload has already happened
            // for every other name.
            let _ = self.remove(&failure.name);
        }
        refused
    }

    /// Calls every plugin, collecting one result each.
    ///
    /// A plugin that errors does not stop the others; its error is returned in place.
    /// The same holds for a plugin that *panics*: the unwind is caught and reported as
    /// a [`Panicked`] error rather than reaching the caller. That does not make the
    /// plugin trustworthy afterwards — see [`Panicked`] — and it cannot help with a
    /// crash that never unwinds, which is what the `remote` feature is for.
    pub fn dispatch<'a, R>(
        &'a self,
        call: impl Fn(&'a C::Instance) -> mlua::Result<R>,
    ) -> Vec<Outcome<'a, R>> {
        self.plugins
            .iter()
            .map(|plugin| {
                // The instruction limit applies per call, not per plugin lifetime.
                if let Some(budget) = &plugin.budget {
                    budget.reset();
                }
                Outcome {
                    name: plugin.name(),
                    // A panicking plugin is reported like a failing one rather than
                    // taking the caller's stack with it.
                    result: panics::guard(|| call(&plugin.instance))
                        .unwrap_or_else(|panicked| Err(panicked.into())),
                }
            })
            .collect()
    }

    /// Awaits every plugin in turn, collecting one result each.
    ///
    /// Calls are sequential: they all reach the same Lua state, so running them
    /// concurrently would only contend on it. A panic in any poll is caught, exactly
    /// as in [`Registry::dispatch`].
    #[cfg(feature = "async")]
    pub async fn dispatch_async<'a, R, Fut>(
        &'a self,
        call: impl Fn(&'a C::Instance) -> Fut,
    ) -> Vec<Outcome<'a, R>>
    where
        Fut: std::future::Future<Output = mlua::Result<R>>,
    {
        let mut outcomes = Vec::with_capacity(self.plugins.len());
        for plugin in &self.plugins {
            if let Some(budget) = &plugin.budget {
                budget.reset();
            }
            outcomes.push(Outcome {
                name: plugin.name(),
                result: panics::guard_future(call(&plugin.instance))
                    .await
                    .unwrap_or_else(|panicked| Err(panicked.into())),
            });
        }
        outcomes
    }

    /// Unbinds a granted capability from a live plugin.
    ///
    /// The name becomes `nil` in the plugin's environment, so subsequent calls fail
    /// rather than reaching the host. Code that already captured the value in a local
    /// keeps it, so this defangs a misbehaving plugin but does not rewind it.
    pub fn revoke(&mut self, plugin: &str, capability: &str) -> Result<bool, RegistryError> {
        let position = *self
            .index
            .get(plugin)
            .ok_or_else(|| RegistryError::UnknownPlugin(plugin.to_string()))?;
        let entry = self
            .plugins
            .get_mut(position)
            .ok_or_else(|| RegistryError::UnknownPlugin(plugin.to_string()))?;

        let Some(index) = entry.granted.iter().position(|name| name == capability) else {
            return Ok(false);
        };
        entry
            .environment
            .set(capability, mlua::Value::Nil)
            .map_err(RegistryError::Lua)?;
        entry.granted.remove(index);
        Ok(true)
    }

    /// Reports what every plugin under `root` requests, without executing anything.
    ///
    /// This is the review-before-you-run surface: manifests are static, so a host can
    /// see exactly what a plugin wants before any of its code runs. Ambient globals
    /// are listed too, since they reach a plugin whether it declared them or not.
    pub fn audit(&self, root: impl AsRef<Path>) -> Result<Audit, RegistryError> {
        self.check_setup()?;
        let (manifests, unreadable) = manifest::discover(root.as_ref())?;
        let plugins = manifests
            .into_iter()
            .map(|manifest| {
                #[cfg(feature = "signatures")]
                let signer = self
                    .verify_plugin(&manifest)
                    .map_or(Signer::Unsigned, |(signer, _)| signer);

                let requests = manifest
                    .capabilities
                    .iter()
                    .map(|(name, declared)| {
                        let (params, optional) = capability::split_optional(declared);
                        CapabilityRequest {
                            plugin: manifest.name.clone(),
                            #[cfg(feature = "signatures")]
                            signer: signer.clone(),
                            name: name.clone(),
                            params,
                            optional,
                        }
                    })
                    .collect();

                PluginAudit {
                    name: manifest.name,
                    dir: manifest.dir,
                    requests,
                    #[cfg(feature = "signatures")]
                    signer,
                }
            })
            .collect();

        Ok(Audit {
            plugins,
            ambient: self
                .host_setup
                .ambient_labels()
                .map(str::to_string)
                .collect(),
            offered: self.host_setup.offered().map(str::to_string).collect(),
            unreadable,
        })
    }

    /// Looks a plugin up by name.
    pub fn get(&self, name: &str) -> Option<&Plugin<C>> {
        self.index
            .get(name)
            .and_then(|position| self.plugins.get(*position))
    }

    /// Every loaded plugin, in load order.
    pub fn plugins(&self) -> &[Plugin<C>] {
        &self.plugins
    }

    /// Names of every loaded plugin, in load order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.plugins.iter().map(Plugin::name)
    }

    /// Number of loaded plugins.
    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    /// True when no plugin has loaded.
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Applies the rocks tree's module paths once, and lists what it holds.
    #[cfg(feature = "luarocks")]
    fn prepare_rocks(
        &mut self,
    ) -> Result<Option<std::collections::BTreeMap<String, rocks::RockVersion>>, RegistryError> {
        let Some(config) = self.rocks.clone() else {
            return Ok(None);
        };
        if self.rock_paths.is_none() {
            // Cached once: every state the registry creates gets the same paths.
            self.rock_paths = Some(config.paths().map_err(RegistryError::Rocks)?);
        }
        let installed = config.installed().map_err(RegistryError::Rocks)?;
        Ok(Some(installed))
    }

    /// Installs every rock declared by any plugin under `root`.
    ///
    /// Loading never installs anything; call this when the host wants to provision.
    #[cfg(feature = "luarocks")]
    pub fn install_rocks(&self, root: impl AsRef<Path>) -> Result<InstallReport, RegistryError> {
        let config = self.rocks.as_ref().ok_or_else(|| {
            RegistryError::Rocks(rocks::RocksError::Requirement {
                raw: String::new(),
                message: "no LuaRocks tree configured; call `with_rocks` first".to_string(),
            })
        })?;

        let (manifests, _) = manifest::discover(root.as_ref())?;
        let mut wanted: std::collections::BTreeMap<String, String> =
            std::collections::BTreeMap::new();
        for manifest in &manifests {
            for (name, requirement) in &manifest.rocks {
                wanted.insert(name.clone(), requirement.clone());
            }
        }

        let installed = config.installed().map_err(RegistryError::Rocks)?;
        let mut report = InstallReport::default();

        for (name, raw) in wanted {
            let requirement = match rocks::Requirement::parse(&raw) {
                Ok(requirement) => requirement,
                Err(err) => {
                    report.failures.push((name, err.to_string()));
                    continue;
                }
            };
            if installed
                .get(&name)
                .is_some_and(|found| requirement.matches(found))
            {
                report.satisfied.push(name);
                continue;
            }
            match config.install(&name, &requirement) {
                Ok(()) => report.installed.push(name),
                Err(err) => report.failures.push((name, err.to_string())),
            }
        }
        Ok(report)
    }

    /// Evaluates a plugin's chunk and constructs its instance, wired to its
    /// dependencies. Returns the instance and its raw exports table, if it published one.
    fn instantiate(
        &self,
        lua: &Lua,
        runtime: &dyn Runtime,
        budget: Option<&Budget>,
        manifest: &Manifest,
        #[cfg(feature = "signatures")] signer: &Signer,
        #[cfg(feature = "signatures")] digest: Option<&DirectoryDigest>,
    ) -> Result<Loaded<C>, FailureReason> {
        let path = manifest.entry_path();
        let source = fs::read_to_string(&path)?;

        // Confirm these are the bytes that were verified, not whatever is on disk now.
        #[cfg(feature = "signatures")]
        if let Some(digest) = digest {
            let relative = slash_path(&manifest.entry);
            if !digest.matches(&relative, source.as_bytes()) {
                return Err(FailureReason::DigestMismatch(relative));
            }
        }

        extend_package_path(lua, &manifest.dir)?;
        let environment = plugin_environment(lua)?;
        let granted = self.grant_capabilities(
            runtime,
            &environment,
            manifest,
            #[cfg(feature = "signatures")]
            signer,
        )?;
        // Submodules must see the same environment, or a plugin split across files
        // would only half-see its capabilities.
        install_plugin_require(
            lua,
            &environment,
            #[cfg(feature = "signatures")]
            manifest.dir.clone(),
            #[cfg(feature = "signatures")]
            digest.cloned(),
        )?;

        // A runaway chunk must not hang the load either.
        if let Some(budget) = budget {
            budget.reset();
        }

        let class: C = lua
            .load(source)
            .set_name(path.display().to_string())
            .set_environment(environment.clone())
            .eval()?;

        let config = lua.to_value(&manifest.config)?;
        let dependencies = self.dependency_table(lua, manifest)?;
        let instance: C::Instance = class
            .handle()
            .call_function(&self.constructor, (config, dependencies))?;

        let exports = self.extract_exports(instance.handle())?;
        Ok(Loaded {
            instance,
            exports,
            environment,
            granted,
        })
    }

    /// Verifies a plugin directory and reports who signed it.
    ///
    /// A missing signature is not fraud: it becomes [`Signer::Unsigned`] unless the
    /// registry requires signatures, so hosts can adopt signing incrementally.
    #[cfg(feature = "signatures")]
    pub fn verify_plugin(
        &self,
        manifest: &Manifest,
    ) -> Result<(Signer, Option<DirectoryDigest>), FailureReason> {
        // A digest is needed to verify a signature, and also to match a denylist
        // entry, so it is computed whenever either is configured.
        let wants_digest = self.verifier.is_some()
            || self.lockfile.is_some()
            || self
                .revocations
                .as_ref()
                .is_some_and(|list| !list.is_empty());
        let digest = if wants_digest {
            Some(DirectoryDigest::compute(&manifest.dir)?)
        } else {
            None
        };

        let signer = match (&self.verifier, &digest) {
            (Some(verifier), Some(digest)) => match verifier.verify(digest, &manifest.dir) {
                Ok(signer) => signer,
                Err(VerifyError::Missing) if !self.require_signatures => Signer::Unsigned,
                Err(VerifyError::Missing) => return Err(FailureReason::Unsigned),
                Err(err @ VerifyError::Untrusted(_)) => {
                    return Err(FailureReason::UntrustedSigner(err.to_string()));
                }
                Err(VerifyError::Io(source)) => return Err(FailureReason::Io(source)),
                Err(err) => return Err(FailureReason::SignatureInvalid(err.to_string())),
            },
            _ if self.require_signatures => return Err(FailureReason::Unsigned),
            _ => Signer::Unsigned,
        };

        // After verification, not instead of it: being revoked is a fact about a
        // signature that is otherwise perfectly valid.
        if let Some(list) = &self.revocations
            && let Some(reason) = list.check(digest.as_ref(), &signer)
        {
            return Err(FailureReason::Revoked(reason));
        }

        // Last, because a pin is a statement about a plugin already established to be
        // what it claims: the digest confirms the bytes, verification decides the
        // signer, and this decides whether that pairing is the one the host wrote down.
        if let Some(lockfile) = &self.lockfile
            && let Some(digest) = &digest
            && let Err(err) = lockfile.check(manifest, digest, &signer)
        {
            return Err(FailureReason::Lock(err));
        }

        Ok((signer, digest))
    }

    /// Resolves and binds one plugin's declared capabilities into its environment.
    ///
    /// Denial of a required capability fails the plugin; denial of an optional one
    /// simply leaves the name unbound.
    fn grant_capabilities(
        &self,
        runtime: &dyn Runtime,
        _environment: &Table,
        manifest: &Manifest,
        #[cfg(feature = "signatures")] signer: &Signer,
    ) -> Result<Vec<String>, FailureReason> {
        let mut granted = Vec::new();

        for (name, declared) in &manifest.capabilities {
            let (params, optional) = capability::split_optional(declared);
            let request = CapabilityRequest {
                plugin: manifest.name.clone(),
                #[cfg(feature = "signatures")]
                signer: signer.clone(),
                name: name.clone(),
                params,
                optional,
            };

            let Some(provider) = self.host_setup.provider(name) else {
                if optional {
                    continue;
                }
                return Err(FailureReason::UnknownCapability(name.clone()));
            };

            // Deny by default: offering a capability is not granting it.
            let decision = match &self.policy {
                Some(policy) => policy.decide(&request),
                None => Decision::Deny("no capability policy is configured".to_string()),
            };

            let approved = match decision {
                Decision::Grant => request.params.clone(),
                Decision::GrantWith(params) => params,
                Decision::Deny(reason) => {
                    if optional {
                        continue;
                    }
                    return Err(FailureReason::CapabilityDenied {
                        name: name.clone(),
                        reason,
                    });
                }
            };

            let grant = Grant::new(manifest.name.clone(), name.clone(), approved);
            let value = provider(runtime, &grant)?;
            _environment.set(name.as_str(), value)?;
            granted.push(name.clone());
        }

        Ok(granted)
    }

    /// Evaluates policy for a plugin's declared capabilities (non-Lua path).
    pub fn evaluate_policy(
        &self,
        manifest: &Manifest,
        #[cfg(feature = "signatures")] signer: &Signer,
    ) -> Vec<String> {
        let mut granted = Vec::new();
        for (name, declared) in &manifest.capabilities {
            let (params, optional) = capability::split_optional(declared);
            let request = CapabilityRequest {
                plugin: manifest.name.clone(),
                #[cfg(feature = "signatures")]
                signer: signer.clone(),
                name: name.clone(),
                params,
                optional,
            };
            let Some(_) = self.host_setup.provider(name) else { continue };
            let decision = match &self.policy {
                Some(p) => p.decide(&request),
                None => Decision::Deny("no policy".to_string()),
            };
            if matches!(decision, Decision::Grant | Decision::GrantWith(_)) {
                granted.push(name.clone());
            }
        }
        granted
    }

    /// Builds the `deps` table handed to a constructor: dependency name to proxy.
    ///
    /// Load order guarantees every required dependency is already present.
    fn dependency_table(&self, lua: &Lua, manifest: &Manifest) -> Result<Table, FailureReason> {
        let dependencies = lua.create_table()?;
        for (name, spec) in &manifest.dependencies {
            match self.get(name) {
                Some(plugin) => match plugin.exports() {
                    Some(proxy) => dependencies.set(name.as_str(), proxy.clone())?,
                    None => return Err(FailureReason::MissingExports(name.clone())),
                },
                // An absent optional dependency simply leaves a nil slot.
                None if spec.is_optional() => {}
                None => return Err(FailureReason::MissingDependency(name.clone())),
            }
        }
        Ok(dependencies)
    }

    /// Reads a plugin's `exports`, accepting either a table or a function returning one.
    fn extract_exports(
        &self,
        instance: &stanchion_lua::LuaHandle,
    ) -> Result<Option<Table>, FailureReason> {
        match instance.get::<Value>(EXPORTS_KEY)? {
            Value::Nil => Ok(None),
            Value::Table(table) => Ok(Some(table)),
            Value::Function(function) => Ok(Some(function.call(instance.to_value())?)),
            other => Err(FailureReason::Lua(mlua::Error::RuntimeError(format!(
                "`{EXPORTS_KEY}` must be a table or a function returning one, found {}",
                other.type_name()
            )))),
        }
    }

    /// Wraps an exports table in the stable proxy dependents hold.
    fn make_proxy(&self, lua: &Lua, table: &Table) -> mlua::Result<Exports> {
        let proxy = lua.create_table()?;
        let metatable = lua.create_table()?;
        let exports = Exports { proxy, metatable };
        exports.repoint(table.clone())?;
        exports
            .proxy
            .set_metatable(Some(exports.metatable.clone()))?;
        Ok(exports)
    }
}



/// Checks one plugin's `[rocks]` against what the tree holds.
#[cfg(feature = "luarocks")]
fn verify_rocks(
    manifest: &Manifest,
    installed: Option<&std::collections::BTreeMap<String, rocks::RockVersion>>,
) -> Result<(), FailureReason> {
    if manifest.rocks.is_empty() {
        return Ok(());
    }
    let Some(installed) = installed else {
        return Err(FailureReason::Rocks(
            "declares `[rocks]` but the registry has no LuaRocks tree configured".to_string(),
        ));
    };

    for (name, raw) in &manifest.rocks {
        let requirement =
            rocks::Requirement::parse(raw).map_err(|err| FailureReason::Rocks(err.to_string()))?;
        match installed.get(name) {
            None => {
                return Err(FailureReason::MissingRock {
                    name: name.clone(),
                    required: requirement.to_string(),
                });
            }
            Some(found) if !requirement.matches(found) => {
                return Err(FailureReason::IncompatibleRock {
                    name: name.clone(),
                    required: requirement.to_string(),
                    found: found.to_string(),
                });
            }
            Some(_) => {}
        }
    }
    Ok(())
}

/// Prepends a rocks tree's search paths to the shared state.
#[cfg(feature = "luarocks")]
fn prepend_module_paths(lua: &Lua, paths: &rocks::RockPaths) -> mlua::Result<()> {
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

/// Renders a relative path with `/` separators, matching the digest's keys.
#[cfg(feature = "signatures")]
fn slash_path(path: impl AsRef<Path>) -> String {
    path.as_ref()
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Gives a plugin a `require` that loads its modules into its own environment.
///
/// Lua's stock `require` runs a module chunk in the global environment and caches it
/// in the shared `package.loaded`. Both are wrong here: a submodule would not see the
/// capabilities bound in its plugin's environment, and two plugins could not load
/// different versions of the same module name. This replacement searches the same
/// `package.path`, loads with the plugin's environment, and caches per plugin.
///
/// Anything it cannot find on disk falls through to the original `require`, so
/// preloaded and C modules still resolve.
fn install_plugin_require(
    lua: &Lua,
    environment: &Table,
    #[cfg(feature = "signatures")] plugin_dir: std::path::PathBuf,
    #[cfg(feature = "signatures")] digest: Option<DirectoryDigest>,
) -> mlua::Result<()> {
    let loaded = lua.create_table()?;
    let fallback: Option<mlua::Function> = lua.globals().get("require")?;
    let package: Option<Table> = lua.globals().get("package")?;
    let plugin_env = environment.clone();

    let require = lua.create_function(move |lua, name: String| {
        if let Some(cached) = loaded.get::<Option<Value>>(name.as_str())? {
            return Ok(cached);
        }

        let search: String = match &package {
            Some(package) => package.get("path").unwrap_or_default(),
            None => String::new(),
        };
        let relative = name.replace('.', std::path::MAIN_SEPARATOR_STR);

        for template in search.split(';').filter(|template| !template.is_empty()) {
            let candidate = template.replace('?', &relative);
            let Ok(source) = fs::read_to_string(&candidate) else {
                continue;
            };

            // A submodule inside a verified plugin must match what was signed.
            #[cfg(feature = "signatures")]
            if let Some(digest) = &digest
                && let Ok(relative) = Path::new(&candidate).strip_prefix(&plugin_dir)
            {
                let relative = slash_path(relative);
                if digest.covers(&relative) && !digest.matches(&relative, source.as_bytes()) {
                    return Err(mlua::Error::RuntimeError(format!(
                        "`{relative}` changed between verification and loading"
                    )));
                }
            }

            let value: Value = lua
                .load(source)
                .set_name(candidate)
                .set_environment(plugin_env.clone())
                .eval()?;
            // Lua treats a module returning nothing as `true`.
            let value = if value.is_nil() {
                Value::Boolean(true)
            } else {
                value
            };
            loaded.set(name.as_str(), value.clone())?;
            return Ok(value);
        }

        match &fallback {
            Some(fallback) => fallback.call(name),
            None => Err(mlua::Error::RuntimeError(format!(
                "module `{name}` not found"
            ))),
        }
    })?;

    environment.set("require", require)
}

/// A table that reads through to the real globals but keeps writes to itself.
fn plugin_environment(lua: &Lua) -> mlua::Result<Table> {
    let environment = lua.create_table()?;
    let metatable = lua.create_table()?;
    metatable.set("__index", lua.globals())?;
    environment.set_metatable(Some(metatable))?;
    Ok(environment)
}

/// Lets a plugin `require` its own files without knowing where it was installed.
fn extend_package_path(lua: &Lua, dir: &Path) -> mlua::Result<()> {
    let Some(package) = lua.globals().get::<Option<Table>>("package")? else {
        return Ok(());
    };
    let current: String = package.get("path").unwrap_or_default();
    let addition = format!("{dir}/?.lua;{dir}/?/init.lua", dir = dir.display());
    if !current.contains(&addition) {
        package.set("path", format!("{addition};{current}"))?;
    }
    Ok(())
}

//! A registry of plugins, loaded through runtime backends.
//!
//! Plugins live in `<root>/<plugin>/plugin.toml` alongside their entry
//! artifact. The registry discovers them, orders them by dependency,
//! verifies provenance (signatures, revocations, lockfile), checks
//! `[rocks]`, decides capabilities through [`Policy`], and hands each
//! backend the verified plugins of its type. Backends instantiate and own
//! all runtime state; this crate never touches a backend's native types —
//! every boundary speaks [`stanchion_abi`] interfaces.
//!
//! One plugin failing never stops the others: [`load_dir`](Registry::load_dir)
//! reports both halves in [`LoadReport`] rather than returning at the first
//! problem.

mod error;
#[cfg(feature = "signatures")]
pub mod lock;
mod manifest;
pub mod upgrade;

pub use error::{FailureReason, LoadFailure, RegistryError};
pub use manifest::{discover, read_manifest, resolve_order};
pub use stanchion_abi::manifest::{
    DependencySpec, DetailedDependency, MANIFEST_FILE, Manifest, PluginType,
};
/// Re-exported so hosts can read manifests without naming another dependency.
pub use toml;
pub use stanchion_abi::panics::Panicked;
pub use stanchion_abi::runtime::Runtime;
/// Re-exported capability vocabulary: the registry, hosts and policies all
/// name the same types rather than three that drift.
pub use stanchion_abi::{
    CapabilityCall, CapabilityProvider, CapabilityRequest, Decision, Grant, HostSetup, Policy,
    Rules, Value, OPTIONAL_KEY,
};
#[cfg(feature = "signatures")]
pub use stanchion_abi::signature::{
    BUNDLE_FILE, DirectoryDigest, PluginVerifier, Revocation, Revocations, SIGNATURE_FILE, Signer,
    VerifyError,
};
#[cfg(feature = "signatures")]
pub use lock::{LOCK_FILE, LockError, LockedPlugin, Lockfile};
#[cfg(feature = "luarocks")]
pub use stanchion_abi::rocks;
pub use upgrade::{Change, UpgradeReview};

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::Path;

/// Records one plugin's failure: marks it failed and pushes the reason.
///
/// Every early-`continue` in `load_dir` did these three lines identically.
fn record_failure(
    failed: &mut HashSet<String>,
    failures: &mut Vec<LoadFailure>,
    manifest: &Manifest,
    reason: FailureReason,
) {
    failed.insert(manifest.name.clone());
    failures.push(LoadFailure {
        name: manifest.name.clone(),
        dir: manifest.dir.clone(),
        reason,
    });
}

use stanchion_abi::{GroupOutcome, LoadContext, LoadItem};

/// Constructor looked up on a plugin's class table when none is configured.
pub const DEFAULT_CONSTRUCTOR: &str = "new";

/// A loaded plugin: its manifest and its live instance.
///
/// The instance is opaque — only its backend knows how to drive it. Use
/// [`Registry::call`] and [`Registry::dispatch`] to reach it.
pub struct LoadedPlugin {
    manifest: Manifest,
    instance: Box<dyn stanchion_abi::PluginInstance>,
    granted: Vec<String>,
    #[cfg(feature = "signatures")]
    signer: Signer,
    // Kept so a revocation list arriving after load can be applied without going back
    // to the filesystem, where the bytes may no longer be the ones that were verified.
    #[cfg(feature = "signatures")]
    digest: Option<DirectoryDigest>,
}

impl LoadedPlugin {
    /// The plugin's manifest, including its `[config]` table and directory.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The plugin's name, as declared in its manifest.
    pub fn name(&self) -> &str {
        &self.manifest.name
    }

    /// The live instance, drivable only through its backend's interface.
    pub fn instance(&self) -> &dyn stanchion_abi::PluginInstance {
        &*self.instance
    }

    /// Who signed this plugin.
    #[cfg(feature = "signatures")]
    pub fn signer(&self) -> &Signer {
        &self.signer
    }

    /// The digest of the bytes this plugin was loaded from, when computed.
    ///
    /// `None` when nothing needed it: no verifier, no lockfile and no
    /// revocation list were configured, so the directory was never hashed.
    #[cfg(feature = "signatures")]
    pub fn digest(&self) -> Option<&DirectoryDigest> {
        self.digest.as_ref()
    }

    /// Capabilities this plugin was actually granted, after policy ran.
    pub fn granted_capabilities(&self) -> impl Iterator<Item = &str> {
        self.granted.iter().map(String::as_str)
    }
}

impl fmt::Debug for LoadedPlugin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoadedPlugin")
            .field("name", &self.manifest.name)
            .field("granted", &self.granted)
            .finish_non_exhaustive()
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
            .filter(|request| !self.offered.contains(&request.capability))
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

/// One plugin's result from a dispatch.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    /// The plugin that produced this result.
    pub plugin: String,
    /// Present when the call succeeded.
    pub value: Option<Value>,
    /// Present when it failed. One plugin failing never affects the others.
    pub error: Option<String>,
}

/// A policy decision, boxed for storage.
type PolicyFn = Box<dyn Policy>;

/// A registry of plugins, loaded through runtime backends.
///
/// Backends are selected by each manifest's `plugin_type`. Capabilities and
/// policy cannot be changed after the first plugin loads, which is
/// deliberate: a host that can widen a plugin's reach after that plugin is
/// running has given up the guarantee the whole capability system exists to
/// make. (Enforced by convention: setup and policy are read at load time;
/// `&mut self` is required to replace them.)
pub struct Registry {
    runtimes: Vec<Box<dyn Runtime>>,
    host_setup: HostSetup,
    setup_error: Option<String>,
    policy: PolicyFn,
    constructor: String,
    plugins: Vec<LoadedPlugin>,
    index: HashMap<String, usize>,
    #[cfg(feature = "signatures")]
    verifier: Option<Box<dyn PluginVerifier>>,
    #[cfg(feature = "signatures")]
    require_signatures: bool,
    #[cfg(feature = "signatures")]
    revocations: Option<Revocations>,
    #[cfg(feature = "signatures")]
    lockfile: Option<Lockfile>,
    #[cfg(feature = "luarocks")]
    rocks: Option<stanchion_abi::rocks::RocksConfig>,
    #[cfg(feature = "luarocks")]
    rock_paths: Option<stanchion_abi::rocks::RockPaths>,
}

impl Default for Registry {
    fn default() -> Self {
        Registry {
            runtimes: Vec::new(),
            host_setup: HostSetup::default(),
            setup_error: None,
            policy: Box::new(Rules::deny_all()),
            constructor: DEFAULT_CONSTRUCTOR.to_string(),
            plugins: Vec::new(),
            index: HashMap::new(),
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
}

impl Registry {
    /// Creates an empty registry with a deny-all policy.
    ///
    /// Register backends with [`with_runtime`](Self::with_runtime), offer
    /// capabilities with [`with_setup`](Self::with_setup), then open them
    /// with [`with_policy`](Self::with_policy).
    pub fn new() -> Self {
        Registry::default()
    }

    /// Registers a runtime backend, replacing any previous one for its type.
    ///
    /// Each variant of [`PluginType`] has exactly one backend; the last
    /// registration wins.
    pub fn with_runtime(mut self, backend: Box<dyn Runtime>) -> Self {
        let ty = backend.plugin_type();
        self.runtimes.retain(|b| b.plugin_type() != ty);
        self.runtimes.push(backend);
        self
    }

    /// Declares everything plugins can reach.
    ///
    /// Runs before the first plugin loads. Capabilities registered here are
    /// gated: a plugin gets one only by declaring it and passing [`Policy`].
    /// Anything registered with [`HostSetup::ambient`] is ungated and reaches
    /// every plugin.
    ///
    /// **Additive.** Calling `with_setup` more than once accumulates onto the
    /// capabilities already registered rather than replacing them.
    pub fn with_setup(mut self, setup: impl FnOnce(&mut HostSetup) -> Result<(), String>) -> Self {
        // Collected immediately so `audit` can report the host's offer before
        // any plugin loads; a failure is held and surfaced by the next
        // fallible call.
        let mut host_setup = std::mem::take(&mut self.host_setup);
        match setup(&mut host_setup) {
            Ok(()) => self.host_setup = host_setup,
            Err(err) => self.setup_error = Some(err),
        }
        self
    }

    /// Decides which requested capabilities are actually granted.
    ///
    /// Without a policy every capability is denied, even one with a
    /// registered provider: offering a capability and granting it are
    /// separate decisions.
    pub fn with_policy(mut self, policy: impl Policy + 'static) -> Self {
        self.policy = Box::new(policy);
        self
    }

    /// What the host offers, once setup has run.
    pub fn host_setup(&self) -> &HostSetup {
        &self.host_setup
    }

    /// Checks every plugin's signature before it loads.
    ///
    /// Verification covers every file in the plugin directory, and each
    /// file's hash is re-checked as it is read, so the bytes that run are
    /// the bytes that were verified.
    #[cfg(feature = "signatures")]
    pub fn with_verifier(mut self, verifier: impl PluginVerifier + 'static) -> Self {
        self.verifier = Some(Box::new(verifier));
        self
    }

    /// Whether an unsigned plugin is refused outright.
    ///
    /// When `false` (the default) an unsigned plugin loads as
    /// [`Signer::Unsigned`], and capability policy can still refuse it
    /// privileges — signing becomes a gradient rather than a cliff. When
    /// `true` every plugin must be signed.
    #[cfg(feature = "signatures")]
    pub fn require_signatures(mut self, required: bool) -> Self {
        self.require_signatures = required;
        self
    }

    /// Whether this registry refuses to load an unsigned plugin.
    ///
    /// Always `false` when the `signatures` feature is off, since nothing is
    /// verified.
    pub fn signatures_required(&self) -> bool {
        #[cfg(feature = "signatures")]
        {
            self.require_signatures
        }
        #[cfg(not(feature = "signatures"))]
        {
            false
        }
    }

    /// Refuses builds or signers on a revocation list.
    ///
    /// Checked after verification, because a withdrawn plugin's signature is
    /// still valid — that is precisely why a separate, mutable list is
    /// needed. Works without a verifier too: a digest denylist refuses a
    /// specific build with no signing infrastructure at all.
    #[cfg(feature = "signatures")]
    pub fn with_revocations(mut self, revocations: Revocations) -> Self {
        self.revocations = Some(revocations);
        self
    }

    /// Refuses any plugin whose bytes are not the ones this lockfile pins.
    ///
    /// Every discovered plugin must be pinned. One in the root with no entry
    /// fails as [`LockError::Unlocked`] rather than loading, because once a
    /// host keeps a lockfile, an unpinned directory appearing beside the
    /// pinned ones is exactly the event worth refusing.
    #[cfg(feature = "signatures")]
    pub fn with_lockfile(mut self, lockfile: Lockfile) -> Self {
        self.lockfile = Some(lockfile);
        self
    }

    /// The lockfile this registry enforces, if any.
    #[cfg(feature = "signatures")]
    pub fn lockfile(&self) -> Option<&Lockfile> {
        self.lockfile.as_ref()
    }

    /// Verifies `[rocks]` declarations against a LuaRocks tree.
    ///
    /// Without this, a plugin declaring `[rocks]` fails to load rather than
    /// silently resolving its modules from whatever happens to be on the
    /// machine.
    #[cfg(feature = "luarocks")]
    pub fn with_rocks(mut self, config: stanchion_abi::rocks::RocksConfig) -> Self {
        self.rocks = Some(config);
        self
    }

    /// Uses a different constructor name than `new` for class-based plugins.
    pub fn with_constructor(mut self, name: impl Into<String>) -> Self {
        self.constructor = name.into();
        self
    }

    /// Surfaces a failure from the `with_setup` closure, which ran at build time.
    fn check_setup(&self) -> Result<(), RegistryError> {
        match &self.setup_error {
            Some(err) => Err(RegistryError::Backend(err.clone())),
            None => Ok(()),
        }
    }

    fn runtime_for(&self, ty: &PluginType) -> Option<&dyn Runtime> {
        self.runtimes
            .iter()
            .find(|b| b.plugin_type() == *ty)
            .map(|b| &**b)
    }

    /// Applies the rocks tree's module paths once, and lists what it holds.
    #[cfg(feature = "luarocks")]
    fn prepare_rocks(
        &mut self,
    ) -> Result<
        Option<std::collections::BTreeMap<String, stanchion_abi::rocks::RockVersion>>,
        RegistryError,
    > {
        let Some(config) = self.rocks.clone() else {
            return Ok(None);
        };
        if self.rock_paths.is_none() {
            // Cached once: every state the backends create gets the same paths.
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
        use stanchion_abi::rocks::{Requirement, RocksError};
        let config = self.rocks.as_ref().ok_or_else(|| {
            RegistryError::Rocks(RocksError::Requirement {
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
            let requirement = match Requirement::parse(&raw) {
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

    /// The rock search paths to hand backends, when a tree is configured.
    #[cfg(feature = "luarocks")]
    fn rock_path_list(&self) -> Vec<String> {
        self.rock_paths
            .as_ref()
            .map(|paths| {
                paths
                    .path
                    .split(';')
                    .filter(|template| !template.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The rock search paths to hand backends: none without a tree.
    #[cfg(not(feature = "luarocks"))]
    fn rock_path_list(&self) -> Vec<String> {
        Vec::new()
    }

    /// Discovers, verifies and loads every plugin under `root`.
    ///
    /// Only an unreadable root is fatal. A plugin with a broken manifest, a
    /// failing chunk, a missing dependency, or a place in a dependency cycle
    /// is reported in [`LoadReport::failures`] while the rest still load.
    pub fn load_dir(&mut self, root: impl AsRef<Path>) -> Result<LoadReport, RegistryError> {
        self.check_setup()?;
        let (manifests, mut failures) = manifest::discover(root.as_ref())?;
        let (ordered, order_failures) = manifest::resolve_order(manifests);
        failures.extend(order_failures);

        #[cfg(feature = "luarocks")]
        let installed_rocks = self.prepare_rocks()?;

        // Verify each manifest in load order, keeping what is loadable.
        // Dependency failures short-circuit dependents, mirroring order.
        let mut failed: HashSet<String> = failures.iter().map(|f| f.name.clone()).collect();
        let mut ready: Vec<ReadyItem> = Vec::new();
        for manifest in ordered {
            if self.index.contains_key(&manifest.name) {
                let name = manifest.name.clone();
                record_failure(
                    &mut failed,
                    &mut failures,
                    &manifest,
                    FailureReason::Manifest(format!(
                        "another plugin is already registered as `{name}`"
                    )),
                );
                continue;
            }
            // A required dependency that failed leaves this plugin unwired; an
            // optional one just stays unfilled.
            let unmet = manifest
                .dependencies
                .iter()
                .find(|(name, spec)| !spec.is_optional() && failed.contains(*name))
                .map(|(name, _)| name.clone());
            if let Some(dep) = unmet {
                record_failure(
                    &mut failed,
                    &mut failures,
                    &manifest,
                    FailureReason::DependencyFailed(dep),
                );
                continue;
            }
            #[cfg(feature = "luarocks")]
            if let Err(reason) = verify_rocks(&manifest, installed_rocks.as_ref()) {
                record_failure(&mut failed, &mut failures, &manifest, reason);
                continue;
            }
            #[cfg(not(feature = "luarocks"))]
            if !manifest.rocks.is_empty() {
                record_failure(
                    &mut failed,
                    &mut failures,
                    &manifest,
                    FailureReason::Rocks(
                        "declares `[rocks]` but the `luarocks` feature is not enabled".to_string(),
                    ),
                );
                continue;
            }
            #[cfg(feature = "signatures")]
            let (signer, digest) = match self.verify_plugin(&manifest) {
                Ok(verified) => verified,
                Err(reason) => {
                    record_failure(&mut failed, &mut failures, &manifest, reason);
                    continue;
                }
            };
            // Read the entry once, here, and bind it to the digest before
            // handing it to a backend. Backends build from these exact bytes,
            // so the file cannot be swapped between verification and loading.
            let entry_bytes = match fs::read(manifest.entry_path()) {
                Ok(bytes) => bytes,
                Err(source) => {
                    record_failure(
                        &mut failed,
                        &mut failures,
                        &manifest,
                        FailureReason::Io(source),
                    );
                    continue;
                }
            };
            #[cfg(feature = "signatures")]
            if let Some(digest) = &digest
                && let Err(reason) = bind_entry(&manifest, digest, &entry_bytes)
            {
                record_failure(&mut failed, &mut failures, &manifest, reason);
                continue;
            }
            ready.push(ReadyItem {
                #[cfg(feature = "signatures")]
                signer,
                #[cfg(feature = "signatures")]
                digest,
                manifest,
                entry_bytes,
            });
        }

        // Hand each backend the verified plugins of its type, in load order.
        // One call per backend: dependency wiring needs the whole group.
        let mut loaded = Vec::new();
        let mut groups: Vec<(PluginType, Vec<&ReadyItem>)> = Vec::new();
        for item in &ready {
            match groups
                .iter_mut()
                .find(|(ty, _)| *ty == item.manifest.plugin_type)
            {
                Some((_, members)) => members.push(item),
                None => groups.push((item.manifest.plugin_type.clone(), vec![item])),
            }
        }
        // Deterministic backend order: by debug name of the plugin type.
        groups.sort_by(|a, b| format!("{:?}", a.0).cmp(&format!("{:?}", b.0)));
        for (ty, members) in &groups {
            let Some(backend) = self.runtimes.iter().find(|b| b.plugin_type() == *ty) else {
                for item in members {
                    failed.insert(item.manifest.name.clone());
                    failures.push(LoadFailure {
                        name: item.manifest.name.clone(),
                        dir: item.manifest.dir.clone(),
                        reason: FailureReason::NoBackend(format!(
                            "no backend registered for plugin type '{ty:?}'"
                        )),
                    });
                }
                continue;
            };
            let items: Vec<LoadItem> = members
                .iter()
                .map(|item| LoadItem {
                    manifest: &item.manifest,
                    #[cfg(feature = "signatures")]
                    signer: item.signer.clone(),
                    #[cfg(not(feature = "signatures"))]
                    signer: stanchion_abi::signature::Signer::Unsigned,
                    #[cfg(feature = "signatures")]
                    digest: item.digest.clone(),
                    #[cfg(not(feature = "signatures"))]
                    digest: None,
                    entry_bytes: item.entry_bytes.clone(),
                })
                .collect();
            let by_name: HashMap<&str, &ReadyItem> = members
                .iter()
                .map(|item| (item.manifest.name.as_str(), *item))
                .collect();
            let outcomes = {
                let ctx = LoadContext {
                    setup: &self.host_setup,
                    policy: &*self.policy,
                    rock_paths: self.rock_path_list(),
                    #[cfg(feature = "luarocks")]
                    rocks: self.rocks.as_ref(),
                    #[cfg(not(feature = "luarocks"))]
                    rocks: None,
                    constructor: &self.constructor,
                };
                backend.load_group(&items, &ctx)
            };
            for outcome in outcomes {
                match outcome {
                    GroupOutcome::Loaded {
                        name,
                        instance,
                        granted,
                    } => {
                        let Some(item) = by_name.get(name.as_str()) else {
                            failed.insert(name.clone());
                            failures.push(LoadFailure {
                                name,
                                dir: std::path::PathBuf::new(),
                                reason: FailureReason::Runtime(
                                    "backend reported an outcome for an unknown plugin"
                                        .to_string(),
                                ),
                            });
                            continue;
                        };
                        loaded.push(name.clone());
                        self.index.insert(name, self.plugins.len());
                        self.plugins.push(LoadedPlugin {
                            manifest: item.manifest.clone(),
                            instance,
                            granted,
                            #[cfg(feature = "signatures")]
                            signer: item.signer.clone(),
                            #[cfg(feature = "signatures")]
                            digest: item.digest.clone(),
                        });
                    }
                    GroupOutcome::Failed { name, dir, reason } => {
                        failed.insert(name.clone());
                        failures.push(LoadFailure {
                            name,
                            dir,
                            reason: FailureReason::Runtime(reason),
                        });
                    }
                }
            }
        }

        // A required dependent of a plugin that failed *inside* a backend
        // group was ordered after it but never reported: mark it skipped
        // rather than silently dropping it.
        for item in &ready {
            if failed.contains(&item.manifest.name) || self.index.contains_key(&item.manifest.name)
            {
                continue;
            }
            let unmet = item
                .manifest
                .dependencies
                .iter()
                .find(|(name, spec)| !spec.is_optional() && failed.contains(*name))
                .map(|(name, _)| name.clone());
            let reason = match unmet {
                Some(dep) => FailureReason::DependencyFailed(dep),
                None => FailureReason::Runtime(
                    "backend did not report an outcome".to_string(),
                ),
            };
            failed.insert(item.manifest.name.clone());
            failures.push(LoadFailure {
                name: item.manifest.name.clone(),
                dir: item.manifest.dir.clone(),
                reason,
            });
        }

        Ok(LoadReport { loaded, failures })
    }

    /// Re-reads one plugin from disk.
    ///
    /// A plugin that fails to reload leaves the old instance in place.
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
        let ty = current.manifest.plugin_type.clone();

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
        if manifest.plugin_type != ty {
            return Err(fail(FailureReason::Manifest(format!(
                "plugin changed type from `{ty:?}` to `{:?}`; remove and load it again instead",
                manifest.plugin_type
            ))));
        }
        if let Err(message) = manifest.validate() {
            return Err(fail(FailureReason::Manifest(message)));
        }

        #[cfg(feature = "signatures")]
        let (signer, digest) = self.verify_plugin(&manifest).map_err(&fail)?;
        let entry_bytes =
            fs::read(manifest.entry_path()).map_err(|source| fail(FailureReason::Io(source)))?;
        #[cfg(feature = "signatures")]
        if let Some(digest) = &digest {
            bind_entry(&manifest, digest, &entry_bytes).map_err(&fail)?;
        }

        let backend = self
            .runtime_for(&ty)
            .ok_or_else(|| fail(FailureReason::NoBackend(format!("no backend for `{ty:?}`"))))?;
        let item = LoadItem {
            manifest: &manifest,
            #[cfg(feature = "signatures")]
            signer: signer.clone(),
            #[cfg(not(feature = "signatures"))]
            signer: stanchion_abi::signature::Signer::Unsigned,
            #[cfg(feature = "signatures")]
            digest: digest.clone(),
            #[cfg(not(feature = "signatures"))]
            digest: None,
            entry_bytes,
        };
        let ctx = LoadContext {
            setup: &self.host_setup,
            policy: &*self.policy,
            rock_paths: self.rock_path_list(),
            #[cfg(feature = "luarocks")]
            rocks: self.rocks.as_ref(),
            #[cfg(not(feature = "luarocks"))]
            rocks: None,
            constructor: &self.constructor,
        };
        // The backend returns the grants it bound, so a changed manifest gets fresh
        // grants without the policy being consulted a second time. This used to take
        // the instance alone and recompute the list through `evaluate_policy`, which
        // asked every decision twice and derived this field by a different route than
        // `load_dir` does.
        let stanchion_abi::Reloaded { instance, granted } = match backend.reload_plugin(&item, &ctx)
        {
            Ok(reloaded) => reloaded,
            Err(err) => return Err(fail(FailureReason::Runtime(err.to_string()))),
        };
        let slot = self
            .plugins
            .get_mut(position)
            .ok_or_else(|| RegistryError::UnknownPlugin(name.to_string()))?;
        *slot = LoadedPlugin {
            manifest,
            instance,
            granted,
            #[cfg(feature = "signatures")]
            signer,
            #[cfg(feature = "signatures")]
            digest,
        };
        Ok(())
    }

    /// Unloads one plugin, forgetting everything its backend retained.
    ///
    /// Dependents holding live surfaces keep working until dropped; the name
    /// is free for a later load to reuse cleanly.
    pub fn remove(&mut self, name: &str) -> Result<LoadedPlugin, RegistryError> {
        let position = *self
            .index
            .get(name)
            .ok_or_else(|| RegistryError::UnknownPlugin(name.to_string()))?;
        if position >= self.plugins.len() {
            return Err(RegistryError::UnknownPlugin(name.to_string()));
        }
        let plugin = self.plugins.remove(position);

        // Removal shifts every later plugin down, so the name index is
        // rebuilt rather than patched: an off-by-one here would hand a
        // caller another plugin.
        self.index.clear();
        for (position, plugin) in self.plugins.iter().enumerate() {
            self.index.insert(plugin.manifest.name.clone(), position);
        }
        if let Some(backend) = self.runtime_for(&plugin.manifest.plugin_type) {
            backend.unload(name);
        }
        Ok(plugin)
    }

    /// Replaces the revocation list and unloads every loaded plugin it names.
    ///
    /// Each returned [`LoadFailure`] names a plugin that was unloaded and
    /// why. Consulting the list only at load would let a plugin withdrawn at
    /// noon keep running until something else happens to reload it.
    #[cfg(feature = "signatures")]
    pub fn apply_revocations(&mut self, revocations: Revocations) -> Vec<LoadFailure> {
        use std::borrow::Cow;
        let mut refused: Vec<LoadFailure> = Vec::new();

        if !revocations.is_empty() {
            let needs_digest = revocations.needs_digest();
            for plugin in &self.plugins {
                let digest = match (&plugin.digest, needs_digest) {
                    (Some(digest), _) => Some(Cow::Borrowed(digest)),
                    (None, false) => None,
                    (None, true) => match DirectoryDigest::compute(&plugin.manifest.dir) {
                        Ok(digest) => Some(Cow::Owned(digest)),
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

        // Stored before the removals so a later load is judged by the same list.
        self.revocations = Some(revocations);

        for failure in &refused {
            let _ = self.remove(&failure.name);
        }
        refused
    }

    /// Calls one method on one plugin.
    ///
    /// A panic inside the backend is caught and reported as an error, matching
    /// [`dispatch`](Self::dispatch) and [`call_async`](Self::call_async). The guard
    /// belongs here rather than in each backend: `PluginBackend` is public, so a
    /// backend a host writes itself would otherwise unwind into the caller even
    /// though the workspace's panic policy says nothing does.
    pub fn call(
        &self,
        plugin: &str,
        method: &str,
        args: &[Value],
    ) -> stanchion_abi::Result<Value> {
        let entry = self.entry(plugin)?;
        stanchion_abi::panics::guard(|| entry.instance.call(method, args)).map_err(|panicked| {
            stanchion_abi::Error::Runtime(stanchion_abi::RuntimeError {
                runtime_name: entry.instance.runtime().to_string(),
                error: panicked.to_string(),
            })
        })?
    }

    /// The loaded plugin by name, or [`Error::UnknownPlugin`].
    ///
    /// Shared by [`call`](Self::call) and [`call_async`](Self::call_async), which held
    /// a copy each.
    fn entry(&self, plugin: &str) -> stanchion_abi::Result<&LoadedPlugin> {
        self.index
            .get(plugin)
            .and_then(|position| self.plugins.get(*position))
            .ok_or_else(|| stanchion_abi::Error::UnknownPlugin(plugin.to_string()))
    }

    /// Awaits one method on one plugin.
    ///
    /// Sequential, not concurrent: backends sharing a state would only
    /// contend on it. A panic during resumption is caught per poll and
    /// reported, exactly as for synchronous calls.
    pub async fn call_async(
        &self,
        plugin: &str,
        method: &str,
        args: &[Value],
    ) -> stanchion_abi::Result<Value> {
        let entry = self.entry(plugin)?;
        stanchion_abi::panics::guard_future(entry.instance.call_async(method, args))
            .await
            .map_err(|panicked| {
                stanchion_abi::Error::Runtime(stanchion_abi::RuntimeError {
                    runtime_name: entry.instance.runtime().to_string(),
                    error: panicked.to_string(),
                })
            })?
    }

    /// Calls the same method on every plugin, collecting one result each.
    ///
    /// A plugin that fails reports its error in place rather than ending the
    /// dispatch.
    pub fn dispatch(&self, method: &str, args: &[Value]) -> Vec<Outcome> {
        self.plugins
            .iter()
            .map(|plugin| {
                let result = stanchion_abi::panics::guard(|| plugin.instance.call(method, args));
                match result {
                    Ok(Ok(value)) => Outcome {
                        plugin: plugin.name().to_string(),
                        value: Some(value),
                        error: None,
                    },
                    Ok(Err(err)) => Outcome {
                        plugin: plugin.name().to_string(),
                        value: None,
                        error: Some(err.to_string()),
                    },
                    Err(panicked) => Outcome {
                        plugin: plugin.name().to_string(),
                        value: None,
                        error: Some(panicked.to_string()),
                    },
                }
            })
            .collect()
    }

    /// Awaits the same method on every plugin, in turn.
    pub async fn dispatch_async(&self, method: &str, args: &[Value]) -> Vec<Outcome> {
        let mut outcomes = Vec::with_capacity(self.plugins.len());
        for plugin in &self.plugins {
            let result = stanchion_abi::panics::guard_future(
                plugin.instance.call_async(method, args),
            )
            .await;
            let result = match result {
                Err(panicked) => Err(stanchion_abi::Error::Runtime(
                    stanchion_abi::RuntimeError {
                        runtime_name: plugin.instance.runtime().to_string(),
                        error: panicked.to_string(),
                    },
                )),
                Ok(inner) => inner,
            };
            outcomes.push(match result {
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
        outcomes
    }

    /// Unbinds a granted capability from a live plugin.
    ///
    /// Returns whether the plugin held it. Code that already captured the
    /// value in a local keeps it, so this defangs a misbehaving plugin
    /// without rewinding it.
    pub fn revoke(&mut self, plugin: &str, capability: &str) -> Result<bool, RegistryError> {
        let position = *self
            .index
            .get(plugin)
            .ok_or_else(|| RegistryError::UnknownPlugin(plugin.to_string()))?;
        let granted = {
            let entry = self
                .plugins
                .get(position)
                .ok_or_else(|| RegistryError::UnknownPlugin(plugin.to_string()))?;
            let Some(index) = entry.granted.iter().position(|name| name == capability) else {
                return Ok(false);
            };
            index
        };
        // Asked of the instance, which is what holds the binding. This used to look up
        // the runtime by plugin type and hand it the instance to downcast back to its
        // own concrete type, so revocation silently failed for any backend other than
        // the one that defined the instance.
        let held = {
            let entry = self
                .plugins
                .get(position)
                .ok_or_else(|| RegistryError::UnknownPlugin(plugin.to_string()))?;
            entry.instance.revoke_capability(capability)
        };
        self.plugins
            .get_mut(position)
            .ok_or_else(|| RegistryError::UnknownPlugin(plugin.to_string()))?
            .granted
            .remove(granted);
        Ok(held)
    }

    /// Reports what every plugin under a root asks for, without running any of it.
    ///
    /// This is the call to make before `load` when the plugins are not yet trusted:
    /// it reads manifests and signatures only.
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
                        let (params, optional) =
                            stanchion_abi::callback::split_optional(declared);
                        CapabilityRequest {
                            plugin: manifest.name.clone(),
                            capability: name.clone(),
                            params,
                            optional,
                            #[cfg(feature = "signatures")]
                            signer: signer.to_string(),
                            #[cfg(not(feature = "signatures"))]
                            signer: "unsigned".to_string(),
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
    pub fn get(&self, name: &str) -> Option<&LoadedPlugin> {
        self.index
            .get(name)
            .and_then(|position| self.plugins.get(*position))
    }

    /// Every loaded plugin, in load order.
    pub fn plugins(&self) -> &[LoadedPlugin] {
        &self.plugins
    }

    /// Names of every loaded plugin, in load order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.plugins.iter().map(LoadedPlugin::name)
    }

    /// Number of loaded plugins.
    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    /// True when no plugin has loaded.
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Verifies a plugin directory and reports who signed it.
    ///
    /// A missing signature is not fraud: it becomes [`Signer::Unsigned`]
    /// unless the registry requires signatures, so hosts can adopt signing
    /// incrementally.
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

    /// Applies policy to a manifest's declared capabilities, without loading it.
    ///
    /// Returns the names that would be granted, or the reason the plugin would be
    /// refused. This is the host-visible half, for auditing and for backends that
    /// check grants at call time; the decision itself is
    /// [`stanchion_abi::approve_capabilities`], which is also what a backend runs when
    /// binding, so the two cannot answer differently.
    ///
    /// It used to be a second, independent implementation of that decision, and the
    /// two disagreed: a missing provider and a `Decision::Deny` were a hard refusal
    /// for the backend and a silent skip here, and a `GrantWith` carrying a non-table
    /// was an error there and a grant here. So this could report a plugin as fine
    /// when loading it would refuse it outright — which is the opposite of useful for
    /// the call its own documentation recommends making *before* `load`.
    ///
    /// Returning `Result` rather than a bare `Vec` is part of that: a refusal is no
    /// longer indistinguishable from "granted nothing".
    pub fn evaluate_policy(
        &self,
        manifest: &Manifest,
        #[cfg(feature = "signatures")] signer: &Signer,
    ) -> Result<Vec<String>, String> {
        #[cfg(feature = "signatures")]
        let signer = signer.to_string();
        #[cfg(not(feature = "signatures"))]
        let signer = "unsigned".to_string();
        let approved = stanchion_abi::approve_capabilities(
            manifest,
            &signer,
            &self.host_setup,
            &*self.policy,
        )?;
        Ok(approved.into_iter().map(|a| a.name).collect())
    }
}

/// One verified manifest with its digest-bound entry bytes, awaiting a backend.
struct ReadyItem {
    manifest: Manifest,
    entry_bytes: Vec<u8>,
    #[cfg(feature = "signatures")]
    signer: Signer,
    #[cfg(feature = "signatures")]
    digest: Option<DirectoryDigest>,
}

/// Confirms `entry_bytes` are the bytes the digest verified.
#[cfg(feature = "signatures")]
fn bind_entry(
    manifest: &Manifest,
    digest: &DirectoryDigest,
    entry_bytes: &[u8],
) -> Result<(), FailureReason> {
    let relative = slash_path(&manifest.entry);
    if !digest.covers(&relative) {
        return Err(FailureReason::UncoveredFile(relative));
    }
    if !digest.matches(&relative, entry_bytes) {
        return Err(FailureReason::DigestMismatch(relative));
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

/// Checks one plugin's `[rocks]` against what the tree holds.
#[cfg(feature = "luarocks")]
fn verify_rocks(
    manifest: &Manifest,
    installed: Option<&std::collections::BTreeMap<String, stanchion_abi::rocks::RockVersion>>,
) -> Result<(), FailureReason> {
    use stanchion_abi::rocks::Requirement;
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
            Requirement::parse(raw).map_err(|err| FailureReason::Rocks(err.to_string()))?;
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

//! Declared capabilities: what a plugin asks for, what the host grants.
//!
//! A plugin requests capabilities in its manifest; the host declares what it can
//! offer in [`crate::Registry::with_setup`](super::Registry::with_setup); a
//! [`Policy`] decides, per plugin, what is actually granted. Granted values are bound
//! in that plugin's environment, so a plugin cannot reach a capability it did not
//! declare and was not given.
//!
//! # What this does and does not buy
//!
//! The grant is baked into the value the provider builds, so a plugin cannot widen it
//! at call time. It bounds *reach*, not *use*: a plugin granted one host can still
//! hammer that host, and a provider that ignores its [`Grant`] makes the declaration
//! decorative. Enforcement lives in the provider.

use std::collections::BTreeMap;
use std::fmt;

use stanchion_abi::runtime::Runtime;
use stanchion_abi::Value;

/// Manifest key reserved by the registry rather than passed to a provider.
pub const OPTIONAL_KEY: &str = "optional";

/// The approved parameters for one capability.
///
/// These are what the [`Policy`] allowed, which may be narrower than what the plugin
/// asked for. A provider should build its value from these and nothing else.
#[derive(Debug, Clone)]
pub struct Grant {
    plugin: String,
    name: String,
    params: toml::Table,
}

impl Grant {
    pub(crate) fn new(plugin: String, name: String, params: toml::Table) -> Self {
        Grant {
            plugin,
            name,
            params,
        }
    }

    /// The plugin this grant was issued to.
    pub fn plugin(&self) -> &str {
        &self.plugin
    }

    /// The capability name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The approved parameters, as they will be seen by the provider.
    pub fn params(&self) -> &toml::Table {
        &self.params
    }

    /// Reads one approved parameter, or `None` if it was not granted.
    pub fn get<T: serde::de::DeserializeOwned>(&self, key: &str) -> Option<T> {
        self.params
            .get(key)
            .cloned()
            .and_then(|value| value.try_into().ok())
    }

    /// Reads one approved parameter, or its default when absent.
    pub fn get_or_default<T: serde::de::DeserializeOwned + Default>(&self, key: &str) -> T {
        self.get(key).unwrap_or_default()
    }
}

/// What a plugin asked for, before any policy ran.
#[derive(Debug, Clone)]
pub struct CapabilityRequest {
    /// Plugin making the request.
    pub plugin: String,
    /// Who signed the plugin, so policy can grant by provenance.
    ///
    /// This is what lets signing tier privileges rather than answer yes or no: a
    /// first-party signature can earn `network` where an unsigned plugin cannot.
    #[cfg(feature = "signatures")]
    pub signer: crate::signature::Signer,
    /// Capability name.
    pub name: String,
    /// Parameters declared in the manifest, with `optional` removed.
    pub params: toml::Table,
    /// Whether the plugin still loads if this is denied.
    pub optional: bool,
}

impl CapabilityRequest {
    /// Who signed the requesting plugin.
    #[cfg(feature = "signatures")]
    pub fn signer(&self) -> &crate::signature::Signer {
        &self.signer
    }
}

/// A policy's answer to one request.
#[derive(Debug, Clone)]
pub enum Decision {
    /// Grant exactly what was asked for.
    Grant,
    /// Grant, but with these parameters instead of the requested ones.
    ///
    /// This is the point of the whole design: a host that can only say yes or no to
    /// `hosts = ["*"]` is a rubber stamp.
    GrantWith(toml::Table),
    /// Refuse, with a reason the plugin author will see.
    Deny(String),
}

impl Decision {
    /// Denies with a stock reason.
    pub fn deny(reason: impl Into<String>) -> Self {
        Decision::Deny(reason.into())
    }
}

/// Decides what each plugin may actually have.
///
/// The registry denies by default: a capability with a registered provider is still
/// refused unless a policy grants it.
pub trait Policy: Send + Sync {
    /// Rules on one request.
    fn decide(&self, request: &CapabilityRequest) -> Decision;
}

impl<F> Policy for F
where
    F: Fn(&CapabilityRequest) -> Decision + Send + Sync + 'static,
{
    fn decide(&self, request: &CapabilityRequest) -> Decision {
        self(request)
    }
}

/// A policy built from per-capability rules.
///
/// ```ignore
/// Rules::deny_all()
///     .allow("log")
///     .allow_with("network", |request| {
///         Decision::GrantWith(toml::toml! { hosts = ["api.example.com"] })
///     })
/// ```
#[derive(Default)]
pub struct Rules {
    rules: BTreeMap<String, RuleFn>,
}

#[cfg(feature = "send")]
type RuleFn = Box<dyn Fn(&CapabilityRequest) -> Decision + Send + Sync>;
#[cfg(not(feature = "send"))]
type RuleFn = Box<dyn Fn(&CapabilityRequest) -> Decision + Send + Sync>;

impl Rules {
    /// A policy that refuses everything; add rules to open specific capabilities.
    pub fn deny_all() -> Self {
        Rules::default()
    }

    /// Grants this capability exactly as requested, to any plugin.
    pub fn allow(self, name: impl Into<String>) -> Self {
        self.allow_with(name, |_| Decision::Grant)
    }

    /// Decides this capability per request, so parameters can be narrowed.
    pub fn allow_with(
        mut self,
        name: impl Into<String>,
        rule: impl Fn(&CapabilityRequest) -> Decision + Send + Sync + 'static,
    ) -> Self {
        self.rules.insert(name.into(), Box::new(rule));
        self
    }
}

impl Policy for Rules {
    fn decide(&self, request: &CapabilityRequest) -> Decision {
        match self.rules.get(&request.name) {
            Some(rule) => rule(request),
            None => Decision::Deny(format!("`{}` is not granted by policy", request.name)),
        }
    }
}

impl fmt::Debug for Rules {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Rules")
            .field("capabilities", &self.rules.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[cfg(feature = "send")]
pub type ProviderFn = Box<dyn Fn(&dyn Runtime, &Grant) -> stanchion_abi::Result<stanchion_abi::Value> + Send + Sync>;
/// Builds the stanchion value a granted capability binds to.
#[cfg(not(feature = "send"))]
pub type ProviderFn = Box<dyn Fn(&dyn Runtime, &Grant) -> stanchion_abi::Result<stanchion_abi::Value> + Send + Sync>;

/// Installs a value into every plugin state, ungated.
#[cfg(feature = "send")]
pub type AmbientFn = Box<dyn Fn(&dyn Runtime) -> stanchion_abi::Result<()> + Send + Sync>;
/// Installs a value into every plugin state, ungated.
#[cfg(not(feature = "send"))]
pub type AmbientFn = Box<dyn Fn(&dyn Runtime) -> stanchion_abi::Result<()>>;

/// What the host offers plugins.
///
/// Collected once, before the first plugin loads, by the closure given to
/// [`crate::Registry::with_setup`](super::Registry::with_setup). Everything a plugin can
/// reach is declared here: gated things through [`capability`](Self::capability),
/// ungated things through [`ambient`](Self::ambient).
#[derive(Default)]
pub struct HostSetup {
    providers: BTreeMap<String, ProviderFn>,
    ambient: Vec<(String, AmbientFn)>,
}

impl HostSetup {
    /// Offers a capability plugins may declare.
    ///
    /// Registering it does not grant it: the [`Policy`] still decides, per plugin.
    /// The provider receives the *approved* [`Grant`], which may be narrower than
    /// what the plugin asked for, and should build its value from that alone.
    pub fn capability(
        &mut self,
        name: impl Into<String>,
        provider: impl Fn(&dyn Runtime, &Grant) -> stanchion_abi::Result<stanchion_abi::Value>
        + Send + Sync
        + 'static,
    ) -> &mut Self {
        self.providers.insert(name.into(), Box::new(provider));
        self
    }

    /// Installs something into every plugin state, gated by nothing.
    ///
    /// This is ambient authority: every plugin sees it whether or not it declared
    /// anything. `label` is what [`crate::Registry::audit`](super::Registry::audit) reports,
    /// so a reviewer can see it alongside declared capabilities.
    pub fn ambient(
        &mut self,
        label: impl Into<String>,
        install: impl Fn(&dyn Runtime) -> stanchion_abi::Result<()> + Send + Sync + 'static,
    ) -> &mut Self {
        self.ambient.push((label.into(), Box::new(install)));
        self
    }

    /// Names of every capability the host can provide.
    pub fn offered(&self) -> impl Iterator<Item = &str> {
        self.providers.keys().map(String::as_str)
    }

    /// Labels of everything installed ambiently.
    pub fn ambient_labels(&self) -> impl Iterator<Item = &str> {
        self.ambient.iter().map(|(label, _)| label.as_str())
    }

    pub(crate) fn provider(&self, name: &str) -> Option<&ProviderFn> {
        self.providers.get(name)
    }

    pub(crate) fn install_ambient(&self, runtime: &dyn Runtime) -> stanchion_abi::Result<()> {
        for (_, install) in &self.ambient {
            install(runtime)?;
        }
        Ok(())
    }
}

impl fmt::Debug for HostSetup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostSetup")
            .field("capabilities", &self.offered().collect::<Vec<_>>())
            .field("ambient", &self.ambient_labels().collect::<Vec<_>>())
            .finish()
    }
}

/// Splits the reserved `optional` key off a manifest capability declaration.
pub(crate) fn split_optional(declared: &toml::Table) -> (toml::Table, bool) {
    let mut params = declared.clone();
    let optional = params
        .remove(OPTIONAL_KEY)
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    (params, optional)
}

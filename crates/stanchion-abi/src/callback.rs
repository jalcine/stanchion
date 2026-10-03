//! Foreign-facing capability types.
//!
//! A plugin requests capabilities in its manifest; the host declares what
//! it can offer in [`HostSetup`]; a [`Policy`] decides, per plugin, what is
//! actually granted. Backends bind granted values into each plugin's
//! environment, so a plugin cannot reach a capability it did not declare
//! and was not given.
//!
//! # What this does and does not buy
//!
//! The grant is baked into the value the provider builds, so a plugin cannot
//! widen it at call time. It bounds *reach*, not *use*: a plugin granted one
//! host can still hammer that host, and a provider that ignores its [`Grant`]
//! makes the declaration decorative. Enforcement lives in the provider.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use crate::value::Value;

/// Manifest key reserved by the registry rather than passed to a provider.
pub const OPTIONAL_KEY: &str = "optional";

/// A capability provider: host-side logic a plugin reaches through a grant.
///
/// Providers are pure host logic over [`Value`]s — they never see a
/// backend's native types. The backend wraps them into whatever callable
/// the plugin's runtime needs, capturing the approved [`Grant`] so every
/// call carries its own bounds.
pub trait CapabilityProvider: Send + Sync {
    /// Does whatever the capability offers, and answers the plugin.
    ///
    /// An `Err` surfaces inside the plugin as a catchable runtime error,
    /// so refusing is a normal outcome rather than a fatal one.
    ///
    /// **Do not call back into the registry that invoked this.** A plugin call holds
    /// its runtime's state lock for the duration and that lock is not reentrant, so
    /// re-entering deadlocks. Through `stanchion-ffi` the attempt is refused with
    /// [`Error::Reentrant`](crate::Error::Reentrant); through
    /// `stanchion_registry::Registry` directly there is no guard and it hangs.
    fn invoke(&self, call: &CapabilityCall) -> std::result::Result<Value, String>;
}

/// Any `Fn(&CapabilityCall) -> Result<Value, String>` answers a capability.
impl<F> CapabilityProvider for F
where
    F: Fn(&CapabilityCall) -> std::result::Result<Value, String> + Send + Sync,
{
    fn invoke(&self, call: &CapabilityCall) -> std::result::Result<Value, String> {
        self(call)
    }
}

/// A shared provider answers through the same implementation.
impl<T: CapabilityProvider + ?Sized> CapabilityProvider for Arc<T> {
    fn invoke(&self, call: &CapabilityCall) -> std::result::Result<Value, String> {
        (**self).invoke(call)
    }
}

/// A policy that grants exactly the capabilities it was built from.
///
/// This is what a host uses when the caller supplied an allow-list rather
/// than a policy object, and it matches how `plugin-host` reads its config
/// file.
#[derive(Debug, Clone, Default)]
pub struct AllowList {
    allowed: Vec<String>,
}

impl AllowList {
    /// Grants exactly `allowed`, denying everything else.
    pub fn new(allowed: impl IntoIterator<Item = String>) -> Self {
        AllowList {
            allowed: allowed.into_iter().collect(),
        }
    }
}

impl Policy for AllowList {
    fn decide(&self, request: &CapabilityRequest) -> Decision {
        if self.allowed.iter().any(|name| name == &request.capability) {
            Decision::Grant
        } else {
            Decision::Deny(format!("`{}` is not granted by policy", request.capability))
        }
    }
}

/// Evaluates policy for one plugin's declared capabilities.
///
/// Returns the granted names. Used by backends that check grants without
/// binding (and by hosts auditing what a manifest would receive): a
/// capability with no provider is skipped, a denied required capability
/// fails the whole plugin, and a denied optional one is skipped.
pub fn evaluate_grants(
    setup: &HostSetup,
    policy: &dyn Policy,
    plugin: &str,
    capabilities: &BTreeMap<String, toml::Table>,
    signer: &str,
) -> std::result::Result<Vec<String>, String> {
    let mut granted = Vec::new();
    for (name, declared) in capabilities {
        let (params, optional) = split_optional(declared);
        let request = CapabilityRequest {
            plugin: plugin.to_string(),
            capability: name.clone(),
            params,
            optional,
            signer: signer.to_string(),
        };
        let Some(_) = setup.provider(name) else {
            if optional {
                continue;
            }
            return Err(format!(
                "requests capability `{name}`, which the host does not offer"
            ));
        };
        match policy.decide(&request) {
            Decision::Grant | Decision::GrantWith(_) => granted.push(name.clone()),
            Decision::Deny(reason) => {
                if optional {
                    continue;
                }
                return Err(format!("capability `{name}` denied: {reason}"));
            }
        }
    }
    Ok(granted)
}

/// What a plugin asked for, as a policy sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct CapabilityRequest {
    /// Plugin making the request.
    pub plugin: String,
    /// Capability name.
    pub capability: String,
    /// Parameters declared in the plugin's manifest, with `optional` removed.
    pub params: Value,
    /// Whether the plugin still loads if this is denied.
    pub optional: bool,
    /// Who signed the plugin, rendered for display (`"unsigned"` when none).
    ///
    /// This is what lets a policy tier privilege by provenance rather than
    /// answering a flat yes or no — a first-party signature can earn
    /// `network` where an unsigned plugin cannot.
    pub signer: String,
}

/// A policy's answer to one request.
#[derive(Clone, Debug, PartialEq)]
pub enum Decision {
    /// Grant exactly what was asked for.
    Grant,
    /// Grant, but with these parameters instead of the requested ones.
    ///
    /// Must be a [`Value::Map`]; capability parameters are a table on both
    /// sides. This is the point of the whole design: a host that can only
    /// say yes or no to `hosts = ["*"]` is a rubber stamp.
    GrantWith(Value),
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
/// The registry denies by default: a capability with a registered provider
/// is still refused unless a policy grants it.
pub trait Policy: Send + Sync {
    /// Rules on one request.
    fn decide(&self, request: &CapabilityRequest) -> Decision;
}

impl<F> Policy for F
where
    F: Fn(&CapabilityRequest) -> Decision + Send + Sync,
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
///         Decision::GrantWith(single_host(request))
///     })
/// ```
#[derive(Default)]
pub struct Rules {
    rules: BTreeMap<String, RuleFn>,
}

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
        match self.rules.get(&request.capability) {
            Some(rule) => rule(request),
            None => Decision::Deny(format!(
                "`{}` is not granted by policy",
                request.capability
            )),
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

/// A plugin reaching back into the host through a granted capability.
#[derive(Clone, Debug, PartialEq)]
pub struct CapabilityCall {
    /// Plugin making the call.
    pub plugin: String,
    /// Capability it was granted.
    pub capability: String,
    /// The parameters policy actually approved, which may be narrower than the
    /// manifest asked for.
    ///
    /// This travels with every call so a provider can re-check its own bounds
    /// rather than trusting the registry to have narrowed correctly.
    pub grant: Value,
    /// Arguments the plugin passed.
    pub args: Vec<Value>,
}

/// A grant, as seen by the runtime when installing a capability.
#[derive(Clone, Debug)]
pub struct Grant {
    /// Plugin this grant was issued to.
    pub plugin: String,
    /// Capability name.
    pub name: String,
    /// Approved parameters.
    pub params: Value,
}

impl Grant {
    /// Creates a new `Grant`.
    pub fn new(plugin: String, name: String, params: Value) -> Self {
        Self {
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

    /// The approved parameters, as the provider sees them.
    pub fn params(&self) -> &Value {
        &self.params
    }
    pub fn get<T: serde::de::DeserializeOwned>(&self, key: &str) -> Option<T> {
        self.params.get(key)
    }
    pub fn get_or_default<T: serde::de::DeserializeOwned + Default>(&self, key: &str) -> T {
        self.params.get_or_default(key)
    }
}

/// What the host offers plugins.
///
/// Collected once, before the first plugin loads. Everything a plugin can
/// reach is declared here: gated things through
/// [`capability`](Self::capability), ungated things through
/// [`ambient`](Self::ambient).
#[derive(Default, Clone)]
pub struct HostSetup {
    providers: BTreeMap<String, Arc<dyn CapabilityProvider>>,
    ambient: Vec<(String, Value)>,
}

impl HostSetup {
    /// Offers a capability plugins may declare.
    ///
    /// Registering it does not grant it: the [`Policy`] still decides, per
    /// plugin. The provider receives the *approved* [`Grant`] on every call,
    /// which may be narrower than what the plugin asked for, and should
    /// enforce those bounds itself.
    pub fn capability(
        &mut self,
        name: impl Into<String>,
        provider: impl CapabilityProvider + 'static,
    ) -> &mut Self {
        self.providers.insert(name.into(), Arc::new(provider));
        self
    }

    /// Installs a value into every plugin state, gated by nothing.
    ///
    /// This is ambient authority: every plugin sees it whether or not it
    /// declared anything. `label` is what audit reports, so a reviewer can
    /// see it alongside declared capabilities.
    pub fn ambient(&mut self, label: impl Into<String>, value: Value) -> &mut Self {
        self.ambient.push((label.into(), value));
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

    /// The provider for one capability, if offered.
    pub fn provider(&self, name: &str) -> Option<&Arc<dyn CapabilityProvider>> {
        self.providers.get(name)
    }

    /// Every ambient value, in registration order.
    pub fn ambient_values(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.ambient
            .iter()
            .map(|(label, value)| (label.as_str(), value))
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

/// Splits the reserved `optional` key off a manifest capability declaration,
/// returning the remaining parameters as a [`Value`] plus the flag.
pub fn split_optional(declared: &toml::Table) -> (Value, bool) {
    let mut params = declared.clone();
    let optional = params
        .remove(OPTIONAL_KEY)
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    (crate::value::table_to_map(&params), optional)
}

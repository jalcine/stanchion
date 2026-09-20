//! Letting foreign code decide policy and answer capabilities.
//!
//! A Rust host configures capabilities with closures. A Python or Kotlin host has
//! none to give, so it implements one of the two traits here and the binding hands
//! over a `dyn` object instead.
//!
//! The pattern is the one `plugin-host` already uses for its forwarded capabilities
//! (`stanchion-remote/src/server.rs`): each capability becomes a Lua function bound
//! in the granted plugin's environment, and calling it reaches back out to whoever
//! provided it. The only difference here is that "out" is a foreign function in this
//! process rather than a JSON-RPC peer in another one.

use std::sync::Arc;

use crate::value::Value;

/// What a plugin asked for, as a foreign policy sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct CapabilityRequest {
    /// Plugin making the request.
    pub plugin: String,
    /// Capability name.
    pub capability: String,
    /// Parameters declared in the plugin's manifest.
    pub params: Value,
    /// Whether the plugin still loads if this is denied.
    pub optional: bool,
    /// Who signed the plugin, rendered for display.
    ///
    /// `"unsigned"` when no signature was present. This is what lets a policy tier
    /// privilege by provenance rather than answering a flat yes or no — a
    /// first-party signature can earn `network` where an unsigned plugin cannot.
    pub signer: String,
}

/// A policy's answer to one request.
#[derive(Clone, Debug, PartialEq)]
pub enum Decision {
    /// Grant exactly what was asked for.
    Grant,
    /// Grant, but with these parameters instead of the requested ones.
    ///
    /// Must be a [`Value::Map`]; capability parameters are a table on both sides.
    GrantWith(Value),
    /// Refuse, with a reason the plugin author will see.
    Deny(String),
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
    /// This travels with every call so a provider can re-check its own bounds rather
    /// than trusting the registry to have narrowed correctly — the same guarantee
    /// the out-of-process host gives its application.
    pub grant: Value,
    /// Arguments the plugin passed.
    pub args: Vec<Value>,
}

/// Decides what each plugin may actually have.
pub trait Policy: Send + Sync {
    /// Rules on one request.
    fn decide(&self, request: &CapabilityRequest) -> Decision;
}

/// Answers a capability a plugin calls.
///
/// **Do not call back into the [`Stanchion`](crate::Stanchion) that invoked this.**
/// The registry is locked for the duration of a plugin call, so re-entering it would
/// deadlock. It does not: the attempt returns [`Error::Reentrant`](crate::Error::Reentrant)
/// instead. Anything a provider needs from the registry should be captured beforehand.
pub trait CapabilityProvider: Send + Sync {
    /// Does whatever the capability offers, and answers the plugin.
    ///
    /// An `Err` surfaces inside Lua as a runtime error the plugin can catch with
    /// `pcall`, so refusing is a normal outcome rather than a fatal one.
    fn invoke(&self, call: &CapabilityCall) -> std::result::Result<Value, String>;
}

impl<F> Policy for F
where
    F: Fn(&CapabilityRequest) -> Decision + Send + Sync,
{
    fn decide(&self, request: &CapabilityRequest) -> Decision {
        self(request)
    }
}

/// A policy that grants exactly the capabilities it was built from.
///
/// This is what a binding uses when the foreign caller supplied an allow-list rather
/// than a policy object, and it matches how `plugin-host` reads its config file.
pub(crate) struct AllowList {
    allowed: Vec<String>,
}

impl AllowList {
    pub(crate) fn new(allowed: impl IntoIterator<Item = String>) -> Self {
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

/// Bridges a foreign [`Policy`] onto the registry's own.
pub(crate) struct PolicyBridge {
    pub(crate) inner: Arc<dyn Policy>,
}

impl stanchion_registry::Policy for PolicyBridge {
    fn decide(
        &self,
        request: &stanchion_registry::CapabilityRequest,
    ) -> stanchion_registry::Decision {
        let foreign = CapabilityRequest {
            plugin: request.plugin.clone(),
            capability: request.name.clone(),
            params: crate::value::table_to_map(&request.params),
            optional: request.optional,
            signer: signer_of(request),
        };

        match self.inner.decide(&foreign) {
            Decision::Grant => stanchion_registry::Decision::Grant,
            Decision::Deny(reason) => stanchion_registry::Decision::Deny(reason),
            Decision::GrantWith(value) => match crate::value::value_to_toml(&value) {
                Ok(toml::Value::Table(table)) => stanchion_registry::Decision::GrantWith(table),
                Ok(_) => stanchion_registry::Decision::Deny(
                    "a narrowed grant must be a map of parameters".to_string(),
                ),
                Err(reason) => stanchion_registry::Decision::Deny(reason),
            },
        }
    }
}

#[cfg(feature = "signatures")]
fn signer_of(request: &stanchion_registry::CapabilityRequest) -> String {
    request.signer.to_string()
}

#[cfg(not(feature = "signatures"))]
fn signer_of(_request: &stanchion_registry::CapabilityRequest) -> String {
    "unverified".to_string()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn an_allow_list_denies_what_it_does_not_name() {
        let policy = AllowList::new(["log".to_string()]);
        let request = |name: &str| CapabilityRequest {
            plugin: "p".to_string(),
            capability: name.to_string(),
            params: Value::Map(Default::default()),
            optional: false,
            signer: "unsigned".to_string(),
        };
        assert_eq!(policy.decide(&request("log")), Decision::Grant);
        assert!(matches!(policy.decide(&request("net")), Decision::Deny(_)));
    }
}

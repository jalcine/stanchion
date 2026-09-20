//! Reviewing what changes when a plugin is upgraded, without running any of its code.
//!
//! [`audit`](crate::Registry::audit) answers "what does this plugin want?" from static
//! manifests. Point the same idea at two versions of one plugin and it answers the
//! question that actually matters when bytes arrive from somewhere else: *did this
//! upgrade quietly ask for more than the version I approved?*
//!
//! That is the realistic supply-chain attack on a plugin system. Stealing a signing
//! key is hard; shipping `1.5.0` of a plugin people already trust, with one more line
//! in `[capabilities]`, is not. A signature does not help — the malicious version is
//! correctly signed by the author whose account was taken. What helps is refusing to
//! install an upgrade that widens authority until a human has read the diff.
//!
//! ```no_run
//! # use stanchion_registry::{Manifest, upgrade::UpgradeReview};
//! # fn example(installed: &Manifest, candidate: &Manifest) {
//! let review = UpgradeReview::between(installed, candidate);
//! if review.widens() {
//!     for concern in review.concerns() {
//!         eprintln!("  {concern}");
//!     }
//!     // refuse, or prompt, but do not install silently
//! }
//! # }
//! ```
//!
//! # What a review does not buy
//!
//! - **It compares declarations, not code.** A plugin that already holds `network` can
//!   change everything it does with it and this diff stays empty. Narrowing the grant
//!   is [policy's](crate::Policy) job; this decides whether to accept new bytes at all.
//! - **Narrowing is inferred conservatively.** Parameters are host-defined, so this
//!   cannot know that `hosts = ["*.acme.com"]` is wider than `["api.acme.com"]`. Any
//!   change it cannot prove is a narrowing counts as a widening, which makes it noisy
//!   rather than quiet.

use std::collections::BTreeSet;
use std::fmt;

use semver::Version;

use crate::capability::{OPTIONAL_KEY, split_optional};
use crate::manifest::Manifest;

/// One difference between two versions of a plugin's declarations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// The upgrade requests a capability the installed version did not.
    CapabilityAdded {
        /// Capability name.
        name: String,
    },
    /// The upgrade no longer requests a capability.
    CapabilityRemoved {
        /// Capability name.
        name: String,
    },
    /// The upgrade requests the same capability with different parameters.
    CapabilityParams {
        /// Capability name.
        name: String,
        /// Rendered parameters the installed version asked for.
        before: String,
        /// Rendered parameters the upgrade asks for.
        after: String,
        /// Whether this crate could prove the new parameters are no wider.
        narrowed: bool,
    },
    /// A capability changed between required and optional.
    CapabilityOptionality {
        /// Capability name.
        name: String,
        /// Whether the installed version declared it optional.
        before: bool,
        /// Whether the upgrade declares it optional.
        after: bool,
    },
    /// The upgrade declares a plugin dependency the installed version did not.
    DependencyAdded {
        /// Dependency name.
        name: String,
        /// The requirement it declares.
        requirement: String,
    },
    /// The upgrade no longer declares a plugin dependency.
    DependencyRemoved {
        /// Dependency name.
        name: String,
    },
    /// The upgrade declares a LuaRocks package the installed version did not.
    ///
    /// Always a concern: rocks are a second supply chain that nothing here signs, and
    /// a C rock is native code outside every guarantee the sandbox makes.
    RockAdded {
        /// Rock name.
        name: String,
        /// The requirement it declares.
        requirement: String,
    },
    /// The upgrade no longer declares a LuaRocks package.
    RockRemoved {
        /// Rock name.
        name: String,
    },
    /// The upgrade evaluates a different entry chunk.
    EntryChanged {
        /// The installed version's entry.
        before: String,
        /// The upgrade's entry.
        after: String,
    },
    /// The upgrade is signed by a different identity, or is no longer signed.
    SignerChanged {
        /// Who signed the installed version.
        before: String,
        /// Who signed the upgrade.
        after: String,
    },
    /// The upgrade's version is not greater than the installed one.
    ///
    /// A downgrade to a signed-but-vulnerable build is a genuine attack that needs no
    /// forgery at all, so it is reported rather than assumed to be a mistake.
    NotNewer {
        /// The installed version.
        before: Version,
        /// The candidate version.
        after: Version,
    },
}

impl Change {
    /// Whether this change grants the plugin reach it did not have before.
    ///
    /// Losing authority is never a widening; gaining or altering it is, unless the
    /// alteration is provably a narrowing.
    pub fn widens(&self) -> bool {
        match self {
            Change::CapabilityAdded { .. }
            | Change::DependencyAdded { .. }
            | Change::RockAdded { .. }
            | Change::EntryChanged { .. }
            | Change::SignerChanged { .. }
            | Change::NotNewer { .. } => true,
            Change::CapabilityParams { narrowed, .. } => !*narrowed,
            Change::CapabilityRemoved { .. }
            | Change::DependencyRemoved { .. }
            | Change::RockRemoved { .. }
            | Change::CapabilityOptionality { .. } => false,
        }
    }
}

impl fmt::Display for Change {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Change::CapabilityAdded { name } => write!(f, "+ capability `{name}`"),
            Change::CapabilityRemoved { name } => write!(f, "- capability `{name}`"),
            Change::CapabilityParams { name, before, after, narrowed } => {
                let marker = if *narrowed { "~" } else { "!" };
                write!(f, "{marker} capability `{name}`: {before} -> {after}")
            }
            Change::CapabilityOptionality { name, before, after } => {
                let render = |optional: bool| if optional { "optional" } else { "required" };
                write!(
                    f,
                    "~ capability `{name}`: {} -> {}",
                    render(*before),
                    render(*after)
                )
            }
            Change::DependencyAdded { name, requirement } => {
                write!(f, "+ dependency `{name}` {requirement}")
            }
            Change::DependencyRemoved { name } => write!(f, "- dependency `{name}`"),
            Change::RockAdded { name, requirement } => {
                write!(f, "+ rock `{name}` {requirement} (unsigned, outside the sandbox)")
            }
            Change::RockRemoved { name } => write!(f, "- rock `{name}`"),
            Change::EntryChanged { before, after } => {
                write!(f, "! entry: {before} -> {after}")
            }
            Change::SignerChanged { before, after } => {
                write!(f, "! signer: {before} -> {after}")
            }
            Change::NotNewer { before, after } => {
                write!(f, "! version {after} does not follow the installed {before}")
            }
        }
    }
}

/// Everything that changed between an installed plugin and a candidate upgrade.
#[derive(Debug, Clone)]
pub struct UpgradeReview {
    /// Plugin name, taken from the installed manifest.
    pub name: String,
    /// The installed version.
    pub from: Version,
    /// The candidate version.
    pub to: Version,
    /// Every difference found, in a stable order.
    pub changes: Vec<Change>,
}

impl UpgradeReview {
    /// Diffs two manifests of the same plugin.
    ///
    /// Signers are compared separately with [`UpgradeReview::with_signers`], since a
    /// manifest cannot state who signed it.
    pub fn between(installed: &Manifest, candidate: &Manifest) -> Self {
        let from = installed.effective_version();
        let to = candidate.effective_version();
        let mut changes = Vec::new();

        if to <= from {
            changes.push(Change::NotNewer { before: from.clone(), after: to.clone() });
        }

        if installed.entry != candidate.entry {
            changes.push(Change::EntryChanged {
                before: installed.entry.clone(),
                after: candidate.entry.clone(),
            });
        }

        diff_capabilities(installed, candidate, &mut changes);

        let names: BTreeSet<&String> = installed
            .dependencies
            .keys()
            .chain(candidate.dependencies.keys())
            .collect();
        for name in names {
            match (installed.dependencies.get(name), candidate.dependencies.get(name)) {
                (None, Some(spec)) => changes.push(Change::DependencyAdded {
                    name: name.clone(),
                    requirement: spec.requirement().to_string(),
                }),
                (Some(_), None) => {
                    changes.push(Change::DependencyRemoved { name: name.clone() })
                }
                _ => {}
            }
        }

        let rocks: BTreeSet<&String> =
            installed.rocks.keys().chain(candidate.rocks.keys()).collect();
        for name in rocks {
            match (installed.rocks.get(name), candidate.rocks.get(name)) {
                (None, Some(requirement)) => changes.push(Change::RockAdded {
                    name: name.clone(),
                    requirement: requirement.clone(),
                }),
                (Some(_), None) => changes.push(Change::RockRemoved { name: name.clone() }),
                _ => {}
            }
        }

        UpgradeReview { name: installed.name.clone(), from, to, changes }
    }

    /// Records that the upgrade is signed by a different identity.
    ///
    /// A plugin changing hands is not itself an attack, but it is never routine, so it
    /// is reported as a widening and left for a human to accept.
    pub fn with_signers(mut self, before: &str, after: &str) -> Self {
        if before != after {
            self.changes.push(Change::SignerChanged {
                before: before.to_string(),
                after: after.to_string(),
            });
        }
        self
    }

    /// Whether the two manifests declare exactly the same things.
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// Whether any change grants the plugin reach it did not have.
    ///
    /// This is the predicate an installer should gate on: a review that does not widen
    /// can be applied unattended, and one that does needs a person.
    pub fn widens(&self) -> bool {
        self.changes.iter().any(Change::widens)
    }

    /// Only the changes that widen, for showing someone who has to decide.
    pub fn concerns(&self) -> impl Iterator<Item = &Change> {
        self.changes.iter().filter(|change| change.widens())
    }
}

impl fmt::Display for UpgradeReview {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{} {} -> {}", self.name, self.from, self.to)?;
        for change in &self.changes {
            writeln!(f, "  {change}")?;
        }
        Ok(())
    }
}

/// Diffs the `[capabilities]` tables, which is where authority actually lives.
fn diff_capabilities(installed: &Manifest, candidate: &Manifest, changes: &mut Vec<Change>) {
    let names: BTreeSet<&String> = installed
        .capabilities
        .keys()
        .chain(candidate.capabilities.keys())
        .collect();

    for name in names {
        match (
            installed.capabilities.get(name),
            candidate.capabilities.get(name),
        ) {
            (None, Some(_)) => changes.push(Change::CapabilityAdded { name: name.clone() }),
            (Some(_), None) => changes.push(Change::CapabilityRemoved { name: name.clone() }),
            (Some(before), Some(after)) => {
                let (before_params, before_optional) = split_optional(before);
                let (after_params, after_optional) = split_optional(after);

                if before_params != after_params {
                    changes.push(Change::CapabilityParams {
                        name: name.clone(),
                        before: render(&before_params),
                        after: render(&after_params),
                        narrowed: narrows(&before_params, &after_params),
                    });
                }
                if before_optional != after_optional {
                    changes.push(Change::CapabilityOptionality {
                        name: name.clone(),
                        before: before_optional,
                        after: after_optional,
                    });
                }
            }
            (None, None) => {}
        }
    }
}

/// Whether `after` can be *proved* to ask for no more than `before`.
///
/// Parameters are host-defined, so this only recognises the shapes it can reason
/// about: an unchanged value, and an array whose every element already appeared in the
/// corresponding array. Anything else — a new key, a dropped key that was constraining
/// something, a scalar that changed — is treated as a widening, because assuming
/// otherwise would be guessing about a host's semantics in the direction that fails
/// open.
fn narrows(before: &toml::Table, after: &toml::Table) -> bool {
    for (key, new) in after {
        let Some(old) = before.get(key) else {
            return false;
        };
        if old == new {
            continue;
        }
        match (old, new) {
            (toml::Value::Array(old), toml::Value::Array(new)) => {
                if !new.iter().all(|item| old.contains(item)) {
                    return false;
                }
            }
            _ => return false,
        }
    }
    // A key that constrained the old request and is gone from the new one is a
    // widening, not a simplification.
    before.keys().all(|key| key == OPTIONAL_KEY || after.contains_key(key))
}

/// Renders a parameter table on one line, for a diff a person reads.
fn render(params: &toml::Table) -> String {
    if params.is_empty() {
        return "{}".to_string();
    }
    let rendered: Vec<String> = params
        .iter()
        .map(|(key, value)| format!("{key} = {value}"))
        .collect();
    format!("{{ {} }}", rendered.join(", "))
}

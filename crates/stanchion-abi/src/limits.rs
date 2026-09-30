//! Backend-neutral resource ceilings for plugin execution.
//!
//! Every backend interprets these in its own native terms (`Sandbox` for
//! Lua, fuel + linear-memory caps for WASM), so hosts and manifests can
//! declare budgets without naming a runtime.

/// Resource ceilings a plugin runs under.
///
/// `None` leaves the backend's default in place. Hosts set the ceiling;
/// a plugin manifest may only *lower* it, never raise it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ResourceLimits {
    /// Ceiling on the plugin's memory, in bytes.
    pub memory_bytes: Option<usize>,
    /// Ceiling on work per call, in backend-native units (VM
    /// instructions, fuel, …).
    pub max_instructions: Option<u64>,
}

impl ResourceLimits {
    /// No overrides; backends apply their own secure defaults.
    pub fn inherit() -> Self {
        ResourceLimits::default()
    }

    /// Combines host and manifest ceilings, taking the lower of each.
    ///
    /// A manifest may narrow what the host allows but never widen it.
    pub fn narrowed_by(mut self, manifest: Option<&crate::manifest::Budget>) -> Self {
        if let Some(budget) = manifest {
            self.max_instructions = Some(match self.max_instructions {
                Some(host) => host.min(budget.max_instructions),
                None => budget.max_instructions,
            });
        }
        self
    }
}

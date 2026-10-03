//! Resource ceilings expressed without naming a runtime.
//!
//! A backend that takes its ceilings this way translates them into its own native
//! terms — [`LuaBackend::shared_with_limits`][lua] and its grouped sibling turn them
//! into a `Sandbox`. A backend is not obliged to: `stanchion-wasm` has its own
//! `WasmLimits` (wasmtime fuel plus a `StoreLimits` memory cap) and never sees a
//! [`ResourceLimits`], because its ceilings are set when the backend is built rather
//! than per load. This module used to claim WASM interpreted these; it does not.
//!
//! What *is* shared across backends is the manifest side: both read
//! [`Budget`](crate::manifest::Budget) and may only narrow a host's ceiling with it,
//! never widen it.
//!
//! [lua]: https://docs.rs/stanchion-lua/latest/stanchion_lua/backend/struct.LuaBackend.html

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
    /// A manifest may narrow what the host allows but never widen it. Both ceilings
    /// narrow: this handled `max_instructions` only, so a manifest's memory ceiling
    /// was dropped on the floor.
    pub fn narrowed_by(mut self, manifest: Option<&crate::manifest::Budget>) -> Self {
        let Some(budget) = manifest else {
            return self;
        };
        self.max_instructions = Some(match self.max_instructions {
            Some(host) => host.min(budget.max_instructions),
            None => budget.max_instructions,
        });
        if let Some(requested) = budget.memory_bytes {
            self.memory_bytes = Some(match self.memory_bytes {
                Some(host) => host.min(requested),
                None => requested,
            });
        }
        self
    }
}

mod runtime;
pub use runtime::{WasmInstance, WasmLimits, WasmRuntime};

use std::path::Path;
use std::sync::Mutex;

use stanchion_abi::{
    Error, GroupOutcome, LoadContext, LoadItem, Manifest, PluginBackend, PluginInstance,
    PluginType, Result, Runtime, Value,
};

/// A WASM plugin backend, registered on a registry with `with_runtime`.
///
/// Loads plugins from `.wasm` binaries specified in the manifest's
/// `entry` field. Each plugin calls its default entry-point export, chosen from the
/// module's exports, with the arguments passed to [`PluginInstance::call`].
///
/// Every instance runs under [`WasmLimits`]: a fuel ceiling (so an infinite loop
/// traps rather than hangs) and a linear-memory ceiling. The limits are the host's,
/// set when the backend is registered; a plugin manifest's `[budget]` may only
/// *lower* them, never raise them — both `max_instructions` (fuel) and
/// `memory_bytes`. See #34.
pub struct WasmBackend {
    limits: WasmLimits,
}

impl Default for WasmBackend {
    fn default() -> Self {
        WasmBackend::new()
    }
}

impl WasmBackend {
    /// A backend with the secure default limits ([`WasmLimits::default`]).
    pub fn new() -> Self {
        WasmBackend {
            limits: WasmLimits::default(),
        }
    }

    /// A backend with host-chosen limits.
    pub fn with_limits(limits: WasmLimits) -> Self {
        WasmBackend { limits }
    }

    /// The limits to load a given plugin under: the host's, with the manifest allowed
    /// only to lower them.
    fn limits_for(&self, manifest: &Manifest) -> WasmLimits {
        let mut limits = self.limits;
        let Some(budget) = &manifest.budget else {
            return limits;
        };
        let requested = budget.max_instructions;
        limits.max_fuel = Some(match limits.max_fuel {
            Some(host) => host.min(requested),
            None => requested,
        });
        // `Budget::memory_bytes` narrows the linear-memory cap the same way fuel is
        // narrowed. Only fuel was read before, so a manifest promising a smaller
        // footprint got the host's ceiling regardless.
        if let Some(bytes) = budget.memory_bytes {
            limits.memory_limit = Some(match limits.memory_limit {
                Some(host) => host.min(bytes),
                None => bytes,
            });
        }
        limits
    }
}

impl PluginBackend for WasmBackend {
    fn plugin_type(&self) -> PluginType {
        PluginType::Wasm
    }

    fn load(&self, manifest: &Manifest, dir: &Path) -> Result<Box<dyn PluginInstance>> {
        let wasm_path = dir.join(&manifest.entry);
        let wasm_binary = std::fs::read(&wasm_path).map_err(|e| Error::Plugin {
            plugin: manifest.name.clone(),
            reason: format!("Failed to read {}: {}", wasm_path.display(), e),
        })?;
        self.compile(manifest, &wasm_binary)
    }

    /// Compiles from the bytes the host already read and verified against the digest,
    /// so the module that runs is exactly the one that was hashed (see #35).
    fn load_bytes(
        &self,
        manifest: &Manifest,
        _dir: &Path,
        entry_bytes: &[u8],
    ) -> Result<Box<dyn PluginInstance>> {
        self.compile(manifest, entry_bytes)
    }
}

impl WasmBackend {
    /// Builds a [`WasmPluginInstance`] from module bytes.
    fn compile(&self, manifest: &Manifest, wasm_binary: &[u8]) -> Result<Box<dyn PluginInstance>> {
        let runtime = WasmRuntime::new(wasm_binary, self.limits_for(manifest)).map_err(|e| {
            Error::Plugin {
                plugin: manifest.name.clone(),
                reason: e,
            }
        })?;

        // Pick the entry point deterministically. `exports()` iterates a HashMap, whose
        // order is unspecified, so "the first export" could resolve to a different
        // function on each load of a multi-export module. Prefer a conventional entry
        // name, then fall back to the lexicographically smallest, so which function runs
        // by default is stable and predictable. See #47.
        let entry = choose_entry(runtime.exports()).ok_or_else(|| Error::Plugin {
            plugin: manifest.name.clone(),
            reason: format!("No exported functions in {}", manifest.entry),
        })?;

        Ok(Box::new(WasmPluginInstance {
            runtime: Mutex::new(runtime),
            entry,
        }))
    }
}

/// Chooses a stable default entry point from a module's exported function names.
///
/// A conventional entry (`_start`, then `main`) wins if present; otherwise the
/// lexicographically smallest name, so the choice never depends on hash iteration
/// order. Returns `None` when the module exports no functions.
fn choose_entry<'a>(exports: impl Iterator<Item = &'a str>) -> Option<String> {
    let mut names: Vec<&str> = exports.collect();
    names.sort_unstable();
    for conventional in ["_start", "main"] {
        if names.contains(&conventional) {
            return Some(conventional.to_string());
        }
    }
    names.first().map(|name| name.to_string())
}

/// A WASM plugin instance wrapping a [`WasmRuntime`].
pub struct WasmPluginInstance {
    runtime: Mutex<WasmRuntime>,
    entry: String,
}

impl Runtime for WasmBackend {
    fn runtime_name(&self) -> &'static str {
        "wasm"
    }

    fn load_group(&self, items: &[LoadItem], ctx: &LoadContext) -> Vec<GroupOutcome> {
        // WASM exports carry no dependency surfaces: each plugin loads
        // independently, and a failure is reported in place.
        items
            .iter()
            .map(|item| {
                let fail = |reason: String| GroupOutcome::Failed {
                    name: item.manifest.name.clone(),
                    dir: item.manifest.dir.clone(),
                    reason,
                };
                let granted = match stanchion_abi::callback::evaluate_grants(
                    ctx.setup,
                    ctx.policy,
                    &item.manifest.name,
                    &item.manifest.capabilities,
                    &item.signer.to_string(),
                ) {
                    Ok(granted) => granted,
                    Err(reason) => return fail(reason),
                };
                match self.compile(item.manifest, &item.entry_bytes) {
                    Ok(instance) => GroupOutcome::Loaded {
                        name: item.manifest.name.clone(),
                        instance,
                        granted,
                    },
                    Err(err) => fail(err.to_string()),
                }
            })
            .collect()
    }

    fn reload_plugin(
        &self,
        item: &LoadItem,
        _ctx: &LoadContext,
    ) -> Result<Box<dyn PluginInstance>> {
        self.compile(item.manifest, &item.entry_bytes)
    }

    fn unload(&self, _name: &str) {}
}

impl PluginInstance for WasmPluginInstance {
    // `revoke_capability` is left to the trait default (`false`): a WASM plugin holds
    // no host capability bindings to unbind, so claiming success would be a lie.
    fn call(&self, method: &str, args: &[Value]) -> Result<Value> {
        let mut runtime = self
            .runtime
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // If the caller specifies a method name, use it as the export name;
        // otherwise fall back to the module's default entry-point export.
        let export = if method.is_empty() {
            &self.entry
        } else {
            method
        };
        runtime.call(export, args).map_err(Error::Wasm)
    }

    fn runtime(&self) -> &str {
        "wasm"
    }
}
#[cfg(test)]
mod tests {
    use super::choose_entry;

    /// The chosen entry must not depend on iteration order: the same set of exports,
    /// presented in any order, yields the same entry. See #47.
    #[test]
    fn entry_choice_is_order_independent() {
        let forward = choose_entry(["alpha", "beta", "gamma"].into_iter());
        let reversed = choose_entry(["gamma", "beta", "alpha"].into_iter());
        assert_eq!(forward, reversed);
        assert_eq!(forward.as_deref(), Some("alpha"));
    }

    #[test]
    fn a_conventional_entry_wins_over_a_smaller_name() {
        // `_start` is preferred even though "aaa" sorts first.
        assert_eq!(
            choose_entry(["aaa", "_start", "zzz"].into_iter()).as_deref(),
            Some("_start")
        );
        // `main` is next in line when there is no `_start`.
        assert_eq!(
            choose_entry(["zzz", "main", "aaa"].into_iter()).as_deref(),
            Some("main")
        );
    }

    #[test]
    fn no_exports_yields_none() {
        assert_eq!(choose_entry(std::iter::empty()), None);
    }
}

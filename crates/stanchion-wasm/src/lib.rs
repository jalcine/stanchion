mod runtime;
pub use runtime::{WasmInstance, WasmLimits, WasmRuntime};

use std::path::Path;
use std::sync::Mutex;

use stanchion_abi::{Error, Manifest, PluginBackend, PluginInstance, PluginType, Result, Value};

/// A WASM plugin backend registered with a [`Builder`].
///
/// Loads plugins from `.wasm` binaries specified in the manifest's
/// `entry` field. Each plugin calls its first exported function (the entry point)
/// with the arguments passed to [`PluginInstance::call`].
///
/// Every instance runs under [`WasmLimits`]: a fuel ceiling (so an infinite loop
/// traps rather than hangs) and a linear-memory ceiling. The limits are the host's,
/// set when the backend is registered; a plugin manifest's `budget.max_instructions`
/// may only *lower* the fuel ceiling, never raise it. See #34.
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
    /// only to lower the fuel ceiling.
    fn limits_for(&self, manifest: &Manifest) -> WasmLimits {
        let mut limits = self.limits;
        if let Some(budget) = &manifest.budget {
            let requested = budget.max_instructions;
            limits.max_fuel = Some(match limits.max_fuel {
                Some(host) => host.min(requested),
                None => requested,
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
        let wasm_binary = std::fs::read(&wasm_path).map_err(|e| {
            Error::Plugin {
                plugin: manifest.name.clone(),
                reason: format!("Failed to read {}: {}", wasm_path.display(), e),
            }
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
        let runtime =
            WasmRuntime::new(wasm_binary, self.limits_for(manifest)).map_err(|e| Error::Plugin {
                plugin: manifest.name.clone(),
                reason: e,
            })?;

        // Use the first exported function as the entry point.
        let entry = runtime
            .exports()
            .next()
            .ok_or_else(|| Error::Plugin {
                plugin: manifest.name.clone(),
                reason: format!("No exported functions in {}", manifest.entry),
            })?
            .to_string();

        Ok(Box::new(WasmPluginInstance {
            runtime: Mutex::new(runtime),
            entry,
        }))
    }
}

/// A WASM plugin instance wrapping a [`WasmRuntime`].
pub struct WasmPluginInstance {
    runtime: Mutex<WasmRuntime>,
    entry: String,
}

impl PluginInstance for WasmPluginInstance {
    fn call(&self, method: &str, args: &[Value]) -> Result<Value> {
        let mut runtime = self.runtime.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        // If the caller specifies a method name, use it as the export name;
        // otherwise fall back to the first exported function (the entry point).
        let export = if method.is_empty() { &self.entry } else { method };
        runtime.call(export, args).map_err(Error::Wasm)
    }

    fn runtime(&self) -> &str {
        "wasm"
    }
}
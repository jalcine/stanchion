mod runtime;
pub use runtime::{WasmInstance, WasmRuntime};

use std::path::Path;
use std::sync::Mutex;

use stanchion_abi::{Error, Manifest, PluginBackend, PluginInstance, PluginType, Result, Value};

/// A WASM plugin backend registered with a [`Builder`].
///
/// Loads plugins from `.wasm` binaries specified in the manifest's
/// `entry` field. Each plugin calls its first exported function (the entry point)
/// with the arguments passed to [`PluginInstance::call`].
pub struct WasmBackend;

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
        let runtime = WasmRuntime::new(wasm_binary).map_err(|e| Error::Plugin {
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
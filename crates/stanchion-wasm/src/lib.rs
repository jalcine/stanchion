mod runtime;
pub use runtime::{WasmInstance, WasmRuntime};

use std::path::Path;
use std::sync::Mutex;

use stanchion_ffi::{
    BackendRegistry, PluginBackend, PluginInstance, Value,
};
use stanchion_registry::{Manifest, PluginType};

/// A WASM plugin backend registered with [`Builder::backend`](stanchion_ffi::Builder::backend).
///
/// Loads plugins from `.wasm` binaries specified in the manifest's
/// `entry` field. Each plugin calls its first exported function (the entry point)
/// with the arguments passed to [`PluginInstance::call`].
pub struct WasmBackend;

impl PluginBackend for WasmBackend {
    fn plugin_type(&self) -> PluginType {
        PluginType::Wasm
    }

    fn load(
        &self,
        manifest: &Manifest,
        dir: &Path,
    ) -> Result<Box<dyn PluginInstance>, String> {
        let wasm_path = dir.join(&manifest.entry);
        let wasm_binary = std::fs::read(&wasm_path)
            .map_err(|e| format!("Failed to read {}: {}", wasm_path.display(), e))?;
        let runtime = WasmRuntime::new(&wasm_binary)?;

        // Use the first exported function as the entry point.
        let entry = runtime.exports().next()
            .ok_or_else(|| format!("No exported functions in {}", manifest.entry))?
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
    fn call(&self, method: &str, args: &[Value]) -> Result<Value, String> {
        let mut runtime = self.runtime.lock().map_err(|e| e.to_string())?;
        // If the caller specifies a method name, use it as the export name;
        // otherwise fall back to the first exported function (the entry point).
        let export = if method.is_empty() {
            &self.entry
        } else {
            method
        };
        runtime.call(export, args)
    }

    fn runtime(&self) -> &str {
        "wasm"
    }
}
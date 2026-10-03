//! Tests for [`BackendRegistry`] and [`PluginBackend`] types.

use stanchion_abi::backend::{BackendRegistry, PluginBackend};
use stanchion_abi::manifest::{Manifest, PluginType};
use std::path::Path;

/// A backend that answers for one [`PluginType`] and refuses to load anything.
///
/// There were three of these — `DummyBackend`, `LuaBackend`, `WasmBackend` — with the
/// same `load` body differing only in the name inside the error nobody asserts on.
struct StubBackend {
    ty: PluginType,
}

impl StubBackend {
    fn lua() -> Self {
        StubBackend {
            ty: PluginType::Lua,
        }
    }

    fn wasm() -> Self {
        StubBackend {
            ty: PluginType::Wasm,
        }
    }
}

impl PluginBackend for StubBackend {
    fn plugin_type(&self) -> PluginType {
        self.ty.clone()
    }

    fn load(
        &self,
        _manifest: &Manifest,
        _dir: &Path,
    ) -> stanchion_abi::Result<Box<dyn stanchion_abi::backend::PluginInstance>> {
        Err(stanchion_abi::Error::Plugin {
            plugin: format!("{:?}", self.ty).to_lowercase(),
            reason: "not implemented".to_string(),
        })
    }
}

#[test]
fn new_registry_is_empty() {
    let registry = BackendRegistry::new();
    assert!(registry.is_empty());
    assert_eq!(registry.len(), 0);
}

#[test]
fn register_adds_backend() {
    let mut registry = BackendRegistry::new();
    registry.register(Box::new(StubBackend::lua()));
    assert_eq!(registry.len(), 1);
    assert!(!registry.is_empty());
}

#[test]
fn get_returns_registered_backend() {
    let mut registry = BackendRegistry::new();
    registry.register(Box::new(StubBackend::lua()));
    assert!(registry.get(&PluginType::Lua).is_some());
    assert!(registry.get(&PluginType::Wasm).is_none());
}

#[test]
fn register_replaces_previous_backend_for_same_type() {
    let mut registry = BackendRegistry::new();
    registry.register(Box::new(StubBackend::lua()));
    assert_eq!(registry.len(), 1);

    // Register another Lua backend - replaces the first
    registry.register(Box::new(StubBackend::lua()));
    assert_eq!(registry.len(), 1);
}

#[test]
fn register_allows_multiple_types() {
    let mut registry = BackendRegistry::new();
    registry.register(Box::new(StubBackend::lua()));
    registry.register(Box::new(StubBackend::wasm()));
    assert_eq!(registry.len(), 2);
    assert!(registry.get(&PluginType::Lua).is_some());
    assert!(registry.get(&PluginType::Wasm).is_some());
}

#[test]
fn iter_returns_all_backends() {
    let mut registry = BackendRegistry::new();
    registry.register(Box::new(StubBackend::lua()));
    registry.register(Box::new(StubBackend::wasm()));

    let types: Vec<PluginType> = registry.iter().map(|b| b.plugin_type()).collect();
    assert_eq!(types.len(), 2);
    assert!(types.contains(&PluginType::Lua));
    assert!(types.contains(&PluginType::Wasm));
}

#[test]
fn register_preserves_last_write_for_same_type() {
    let mut registry = BackendRegistry::new();
    registry.register(Box::new(StubBackend::lua()));
    let second_ty = PluginType::Lua;
    registry.register(Box::new(StubBackend {
        ty: second_ty.clone(),
    }));
    assert_eq!(registry.len(), 1);
    assert!(registry.get(&second_ty).is_some());
}

#[test]
fn get_returns_none_for_empty_registry() {
    let registry: BackendRegistry = BackendRegistry::default();
    assert!(registry.get(&PluginType::Lua).is_none());
}

#[test]
fn len_reflects_registered_backends() {
    let mut registry = BackendRegistry::new();
    assert_eq!(registry.len(), 0);
    registry.register(Box::new(StubBackend::lua()));
    assert_eq!(registry.len(), 1);
    registry.register(Box::new(StubBackend::wasm()));
    assert_eq!(registry.len(), 2);
}

#[test]
fn load_returns_error_for_unimplemented_backends() {
    let manifest = Manifest {
        name: "test".to_string(),
        version: None,
        plugin_type: PluginType::Lua,
        entry: "init.lua".to_string(),
        dependencies: Default::default(),
        capabilities: Default::default(),
        rocks: Default::default(),
        config: Default::default(),
        budget: None,
        dir: Path::new("/tmp").to_path_buf(),
    };
    let dir = Path::new("/tmp");

    let lua = StubBackend::lua();
    assert!(lua.load(&manifest, dir).is_err());

    let wasm = StubBackend::wasm();
    assert!(wasm.load(&manifest, dir).is_err());

    let dummy = StubBackend::lua();
    assert!(dummy.load(&manifest, dir).is_err());
}

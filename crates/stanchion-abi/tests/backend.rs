//! Tests for the [`BackendRegistry`] and [`PluginBackend`] types.

use stanchion_abi::backend::{BackendRegistry, PluginBackend};
use stanchion_abi::manifest::{Manifest, PluginType};
use std::path::Path;

struct DummyBackend {
    ty: PluginType,
}

impl PluginBackend for DummyBackend {
    fn plugin_type(&self) -> PluginType {
        self.ty.clone()
    }

    fn load(&self, _manifest: &Manifest, _dir: &Path) -> stanchion_abi::Result<Box<dyn stanchion_abi::backend::PluginInstance>> {
        unimplemented!()
    }
}

struct LuaBackend;
struct WasmBackend;

impl PluginBackend for LuaBackend {
    fn plugin_type(&self) -> PluginType {
        PluginType::Lua
    }

    fn load(&self, _manifest: &Manifest, _dir: &Path) -> stanchion_abi::Result<Box<dyn stanchion_abi::backend::PluginInstance>> {
        unimplemented!()
    }
}

impl PluginBackend for WasmBackend {
    fn plugin_type(&self) -> PluginType {
        PluginType::Wasm
    }

    fn load(&self, _manifest: &Manifest, _dir: &Path) -> stanchion_abi::Result<Box<dyn stanchion_abi::backend::PluginInstance>> {
        unimplemented!()
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
    registry.register(Box::new(LuaBackend));
    assert_eq!(registry.len(), 1);
    assert!(!registry.is_empty());
}

#[test]
fn get_returns_registered_backend() {
    let mut registry = BackendRegistry::new();
    registry.register(Box::new(LuaBackend));
    assert!(registry.get(&PluginType::Lua).is_some());
    assert!(registry.get(&PluginType::Wasm).is_none());
}

#[test]
fn register_replaces_previous_backend_for_same_type() {
    let mut registry = BackendRegistry::new();
    registry.register(Box::new(LuaBackend));
    assert_eq!(registry.len(), 1);

    // Register another Lua backend - replaces the first
    registry.register(Box::new(DummyBackend { ty: PluginType::Lua }));
    assert_eq!(registry.len(), 1);
}

#[test]
fn register_allows_multiple_types() {
    let mut registry = BackendRegistry::new();
    registry.register(Box::new(LuaBackend));
    registry.register(Box::new(WasmBackend));
    assert_eq!(registry.len(), 2);
    assert!(registry.get(&PluginType::Lua).is_some());
    assert!(registry.get(&PluginType::Wasm).is_some());
}

#[test]
fn iter_returns_all_backends() {
    let mut registry = BackendRegistry::new();
    registry.register(Box::new(LuaBackend));
    registry.register(Box::new(WasmBackend));

    let types: Vec<PluginType> = registry.iter().map(|b| b.plugin_type()).collect();
    assert_eq!(types.len(), 2);
    assert!(types.contains(&PluginType::Lua));
    assert!(types.contains(&PluginType::Wasm));
}
//! Tests for the registry interface in stanchion-ffi.

use stanchion_ffi::{BackendRegistry, PluginBackend, PluginInstance, Error, Result, Value};
use std::path::Path;

struct MockPluginInstance;
impl PluginInstance for MockPluginInstance {
    fn call(&self, _method: &str, _args: &[Value]) -> Result<Value> {
        unimplemented!()
    }

    fn runtime(&self) -> &str {
        "mock"
    }
}

struct MockBackend;
impl PluginBackend for MockBackend {
    fn plugin_type(&self) -> &'static str {
        "mock"
    }

    fn load(&self, _manifest: &serde_json::Value, _dir: &Path) -> Result<Box<dyn PluginInstance>> {
        Ok(Box::new(MockPluginInstance))
    }
}

#[test]
fn backend_registry_new_is_empty() {
    let registry = BackendRegistry::new();
    assert!(registry.is_empty());
    assert_eq!(registry.len(), 0);
}

#[test]
fn backend_registry_register_adds_backend() {
    let mut registry = BackendRegistry::new();
    registry.register(Box::new(MockBackend));
    assert_eq!(registry.len(), 1);
    assert!(!registry.is_empty());
}

#[test]
fn backend_registry_get_finds_registered_backend() {
    let mut registry = BackendRegistry::new();
    registry.register(Box::new(MockBackend));
    assert!(registry.get("mock").is_some());
    assert!(registry.get("missing").is_none());
}

#[test]
fn backend_registry_register_replaces_previous() {
    let mut registry = BackendRegistry::new();
    registry.register(Box::new(MockBackend));
    assert_eq!(registry.len(), 1);

    // Register another backend with same type - should replace
    registry.register(Box::new(MockBackend));
    assert_eq!(registry.len(), 1);
}

#[test]
fn backend_registry_iter() {
    let mut registry = BackendRegistry::new();
    registry.register(Box::new(MockBackend));

    let backends: Vec<_> = registry.iter().collect();
    assert_eq!(backends.len(), 1);
    assert_eq!(backends[0].plugin_type(), "mock");
}

#[test]
fn plugin_instance_runtime_method() {
    let instance = MockPluginInstance;
    assert_eq!(instance.runtime(), "mock");
}
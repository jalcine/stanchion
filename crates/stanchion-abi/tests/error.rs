//! Tests for the [`Error`] type and its kind tags.

use stanchion_abi::Error;

#[test]
fn unknown_plugin_kind() {
    let err = Error::UnknownPlugin("foo".to_string());
    assert_eq!(err.kind(), "unknown-plugin");
    assert_eq!(err.to_string(), "no plugin named `foo`");
}

#[test]
fn plugin_kind() {
    let err = Error::Plugin {
        plugin: "foo".to_string(),
        reason: "bad manifest".to_string(),
    };
    assert_eq!(err.kind(), "plugin");
    assert_eq!(err.to_string(), "plugin `foo`: bad manifest");
}

#[test]
fn lua_kind() {
    let err = Error::Lua("syntax error".to_string());
    assert_eq!(err.kind(), "lua");
    assert_eq!(err.to_string(), "syntax error");
}

#[test]
fn wasm_kind() {
    let err = Error::Wasm("invalid module".to_string());
    assert_eq!(err.kind(), "wasm");
    assert_eq!(err.to_string(), "invalid module");
}

#[test]
fn io_kind() {
    let err = Error::Io("file not found".to_string());
    assert_eq!(err.kind(), "io");
    assert_eq!(err.to_string(), "file not found");
}

#[test]
fn config_kind() {
    let err = Error::Config("bad root".to_string());
    assert_eq!(err.kind(), "config");
    assert_eq!(err.to_string(), "configuration: bad root");
}

#[test]
fn capability_kind() {
    let err = Error::Capability {
        capability: "kv".to_string(),
        reason: "refused".to_string(),
    };
    assert_eq!(err.kind(), "capability");
    assert_eq!(err.to_string(), "capability `kv`: refused");
}

#[test]
fn reentrant_kind() {
    let err = Error::Reentrant;
    assert_eq!(err.kind(), "reentrant");
    assert!(err.to_string().contains("deadlock"));
}

#[test]
fn error_is_clone() {
    let err = Error::UnknownPlugin("foo".to_string());
    let cloned = err.clone();
    assert_eq!(err.kind(), cloned.kind());
}

#[test]
fn error_is_partial_eq() {
    let err1 = Error::UnknownPlugin("foo".to_string());
    let err2 = Error::UnknownPlugin("foo".to_string());
    let err3 = Error::UnknownPlugin("bar".to_string());
    assert_eq!(err1, err2);
    assert_ne!(err1, err3);
}

#[test]
fn error_config_constructor() {
    let err = Error::config("bad root");
    assert_eq!(err.kind(), "config");
    assert!(err.to_string().contains("bad root"));
}
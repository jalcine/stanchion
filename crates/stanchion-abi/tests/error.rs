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
    let err = Error::Runtime(stanchion_abi::RuntimeError {
        runtime_name: "lua".to_string(),
        error: "syntax error".to_string(),
    });
    assert_eq!(err.kind(), "runtime");
    assert_eq!(err.to_string(), "lua runtime error: syntax error");
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

#[test]
fn manifest_kind() {
    let err = Error::Manifest {
        path: "plugins/greeter/plugin.toml".to_string(),
        reason: "expected a value".to_string(),
    };
    assert_eq!(err.kind(), "manifest");
    assert_eq!(
        err.to_string(),
        "manifest `plugins/greeter/plugin.toml`: expected a value"
    );
}

/// The enum crosses a C ABI and four binding generators, so every variant has to
/// survive being matched in each of them. `Toml(toml::de::Error)` did not: it put a
/// foreign crate's type in the one enum whose whole job is to be flat, and
/// `stanchion-ffi-c` stopped compiling because it had no arm for it.
#[test]
fn errors_carry_no_foreign_types() {
    let err = Error::Manifest {
        path: "p".to_string(),
        reason: "r".to_string(),
    };
    // Round-tripping through the flat representation a binding sees must be lossless.
    assert_eq!(err.kind(), "manifest");
    assert!(!err.to_string().is_empty());
}

/// `anyhow`, `eyre` and `tracing` all report a cause chain by walking `source()`.
/// `Error` implemented `std::error::Error` with the default `source()`, so a wrapped
/// [`RuntimeError`] was invisible to every one of them: the chain stopped at the
/// outermost message.
#[test]
fn runtime_errors_are_reachable_as_a_source() -> Result<(), Box<dyn std::error::Error>> {
    use std::error::Error as _;

    let err = Error::Runtime(stanchion_abi::RuntimeError {
        runtime_name: "lua".to_string(),
        error: "attempt to index a nil value".to_string(),
    });

    let source = err.source().ok_or("Runtime should expose its RuntimeError")?;
    assert_eq!(
        source.to_string(),
        "lua runtime error: attempt to index a nil value"
    );
    Ok(())
}

/// The flat variants hold strings, not errors; reporting their message twice (once
/// as the error, once as its own cause) is noise.
#[test]
fn flat_variants_have_no_source() {
    use std::error::Error as _;

    assert!(Error::Io("file not found".to_string()).source().is_none());
    assert!(
        Error::Manifest {
            path: "p".to_string(),
            reason: "r".to_string(),
        }
        .source()
        .is_none()
    );
}

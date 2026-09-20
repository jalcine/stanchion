//! The async surface: plugin methods that yield, and the guard that survives it.

#![cfg(feature = "async")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use stanchion_ffi::{CapabilityCall, CapabilityProvider, Stanchion, Value};
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A plugin whose methods are coroutines, which is what `call_async` drives.
const SLEEPER: &str = r#"
local P = {}
P.__index = P

function P.new(config)
  return setmetatable({ tag = config.tag or "sleeper" }, P)
end

function P:tagged()
  coroutine.yield()
  return self.tag
end

function P:double(n)
  coroutine.yield()
  return n * 2
end

function P:boom()
  coroutine.yield()
  error("deliberate")
end

function P:shout(word)
  coroutine.yield()
  return greet(word)
end

return P
"#;

fn write_plugin(root: &Path, name: &str, manifest: &str, source: &str) -> TestResult {
    let dir = root.join(name);
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("plugin.toml"), manifest)?;
    fs::write(dir.join("init.lua"), source)?;
    Ok(())
}

#[tokio::test]
async fn an_async_call_drives_a_yielding_method() -> TestResult {
    let root = TempDir::new()?;
    write_plugin(
        root.path(),
        "sleeper",
        "name = \"sleeper\"\nentry = \"init.lua\"\n\n[config]\ntag = \"awake\"\n",
        SLEEPER,
    )?;

    let host = Stanchion::builder().build()?;
    assert!(host.load(Some(root.path()))?.is_clean());

    assert_eq!(
        host.call_async("sleeper", "tagged", &[]).await?,
        Value::Str("awake".to_string())
    );
    assert_eq!(
        host.call_async("sleeper", "double", &[Value::Int(21)]).await?,
        Value::Int(42)
    );
    Ok(())
}

#[tokio::test]
async fn an_async_dispatch_collects_every_plugin() -> TestResult {
    let root = TempDir::new()?;
    for (name, tag) in [("a", "alpha"), ("b", "beta")] {
        write_plugin(
            root.path(),
            name,
            &format!("name = \"{name}\"\nentry = \"init.lua\"\n\n[config]\ntag = \"{tag}\"\n"),
            SLEEPER,
        )?;
    }

    let host = Stanchion::builder().build()?;
    host.load(Some(root.path()))?;

    let mut tags: Vec<String> = host
        .dispatch_async("tagged", &[])
        .await?
        .into_iter()
        .filter_map(|outcome| match outcome.value {
            Some(Value::Str(tag)) => Some(tag),
            _ => None,
        })
        .collect();
    tags.sort();
    assert_eq!(tags, vec!["alpha".to_string(), "beta".to_string()]);

    // A failure is still reported in place rather than ending the dispatch.
    let outcomes = host.dispatch_async("boom", &[]).await?;
    assert_eq!(outcomes.len(), 2);
    assert!(outcomes.iter().all(|outcome| outcome.error.is_some()));
    Ok(())
}

#[tokio::test]
async fn an_unknown_plugin_is_named_by_the_async_path_too() -> TestResult {
    let host = Stanchion::builder().build()?;
    let err = host
        .call_async("absent", "tagged", &[])
        .await
        .expect_err("no such plugin");
    assert_eq!(err.kind(), "unknown-plugin");
    Ok(())
}

/// Re-enters the registry from inside an async call.
struct Reenters {
    host: Mutex<Option<Arc<Stanchion>>>,
}

impl CapabilityProvider for Reenters {
    fn invoke(&self, _call: &CapabilityCall) -> Result<Value, String> {
        let host = self.host.lock().map_err(|_| "poisoned".to_string())?;
        let host = host.as_ref().ok_or("no host")?;
        match host.call("sleeper", "tagged", &[]) {
            Err(err) => Err(err.to_string()),
            Ok(_) => Err("the re-entry should not have succeeded".to_string()),
        }
    }
}

/// The guard is set per *poll*, not for the lifetime of the future, because a
/// multi-threaded runtime may move the future between polls. This runs on exactly
/// such a runtime so that a regression to a held guard shows up here.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn re_entry_from_an_async_call_is_caught_across_threads() -> TestResult {
    let root = TempDir::new()?;
    write_plugin(
        root.path(),
        "sleeper",
        "name = \"sleeper\"\nentry = \"init.lua\"\n\n[capabilities.greet]\n",
        SLEEPER,
    )?;

    let provider = Arc::new(Reenters {
        host: Mutex::new(None),
    });
    let host = Arc::new(
        Stanchion::builder()
            .capability("greet", Arc::clone(&provider) as Arc<dyn CapabilityProvider>)
            .build()?,
    );
    *provider.host.lock().unwrap() = Some(Arc::clone(&host));
    host.load(Some(root.path()))?;

    // Without a per-poll guard this hangs instead of returning.
    let err = host
        .call_async("sleeper", "shout", &[Value::Str("x".to_string())])
        .await
        .expect_err("re-entry must fail");
    assert!(err.to_string().contains("deadlock"), "{err}");
    Ok(())
}

/// Two separate registries nest legitimately: different locks, so no hazard.
#[tokio::test]
async fn one_registry_may_be_called_from_another_registrys_provider() -> TestResult {
    struct Delegates {
        inner: Arc<Stanchion>,
    }
    impl CapabilityProvider for Delegates {
        fn invoke(&self, _call: &CapabilityCall) -> Result<Value, String> {
            self.inner
                .call("sleeper", "tagged", &[])
                .map_err(|err| err.to_string())
        }
    }

    // Both registries are reached through the *sync* path here, so neither plugin may
    // yield: a coroutine has nothing to yield to there.
    const INNER: &str = r#"
local P = {}
P.__index = P
function P.new(_) return setmetatable({}, P) end
function P:tagged() return "inner" end
return P
"#;
    const OUTER: &str = r#"
local P = {}
P.__index = P
function P.new(_) return setmetatable({}, P) end
function P:shout(word) return greet(word) end
return P
"#;

    let root = TempDir::new()?;
    write_plugin(
        root.path(),
        "sleeper",
        "name = \"sleeper\"\nentry = \"init.lua\"\n",
        INNER,
    )?;
    let caller_root = TempDir::new()?;
    write_plugin(
        caller_root.path(),
        "sleeper",
        "name = \"sleeper\"\nentry = \"init.lua\"\n\n[capabilities.greet]\n",
        OUTER,
    )?;

    let inner = Arc::new(Stanchion::builder().build()?);
    inner.load(Some(root.path()))?;

    let outer = Stanchion::builder()
        .capability(
            "greet",
            Arc::new(Delegates {
                inner: Arc::clone(&inner),
            }) as Arc<dyn CapabilityProvider>,
        )
        .build()?;
    outer.load(Some(caller_root.path()))?;

    assert_eq!(
        outer.call("sleeper", "shout", &[Value::Nil])?,
        Value::Str("inner".to_string())
    );
    Ok(())
}

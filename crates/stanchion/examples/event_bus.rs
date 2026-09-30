//! An event bus whose handlers are plugins on disk.
//!
//! Shows what a registry adds over a bare class binding: manifest discovery,
//! dependency ordering, published `exports`, and failure isolation at both load and
//! dispatch.
//!
//! ```sh
//! cargo run -p stanchion --features lua54,vendored,registry --example event_bus
//! ```

use std::path::PathBuf;

use stanchion::registry::{Registry, Value};
use stanchion_lua::backend::LuaBackend;

fn plugin_root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/plugins"))
}

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut registry = Registry::new().with_runtime(Box::new(LuaBackend::shared()));
    let report = registry.load_dir(plugin_root())?;

    // `counter` depends on `formatter`, so it loads after it. `broken` raises while
    // loading and is reported rather than aborting the others.
    println!("loaded  : {:?}", report.loaded);
    for failure in &report.failures {
        println!(
            "failed  : {} — {}",
            failure.name,
            first_line(&failure.reason.to_string())
        );
    }

    println!("\n-- dispatching an event to every plugin --");
    for outcome in registry.dispatch(
        "handle",
        &[
            Value::Str("deploy".into()),
            Value::Str("v1.4.2".into()),
        ],
    ) {
        match outcome.value {
            Some(Value::Str(text)) => println!("  {:<10} {text}", outcome.plugin),
            Some(other) => println!("  {:<10} answered unexpectedly: {other:?}", outcome.plugin),
            // `grumpy` fails every call; the others still ran.
            None => println!(
                "  {:<10} failed: {}",
                outcome.plugin,
                first_line(outcome.error.as_deref().unwrap_or("unknown"))
            ),
        }
    }

    println!("\n-- dispatching again: `counter` keeps its state --");
    for outcome in registry.dispatch(
        "handle",
        &[Value::Str("rollback".into()), Value::Str("v1.4.1".into())],
    ) {
        if let Some(Value::Str(text)) = outcome.value {
            println!("  {:<10} {text}", outcome.plugin);
        }
    }

    // `counter` sees only what `formatter` published: its answers arrive
    // decorated, which is only possible through the `decorate` export.
    if let Ok(Value::Str(text)) = registry.call(
        "counter",
        "handle",
        &[Value::Str("probe".into()), Value::Str("x".into())],
    ) {
        println!("\ncounter answers through formatter's `decorate`: {text}");
    }

    Ok(())
}

/// Lua errors carry a traceback; an example reads better with just the message.
fn first_line(message: &str) -> &str {
    message.lines().next().unwrap_or(message)
}

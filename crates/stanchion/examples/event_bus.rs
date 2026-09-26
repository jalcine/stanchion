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

use stanchion_lua::lua_class;
use stanchion_lua::mlua::{Lua, Result, Table};
use stanchion::registry::Registry;

/// Every plugin in the bus implements this.
#[lua_class]
pub trait Handler {
    /// The registry calls this with `(config, deps)`.
    fn new(config: Table, deps: Table) -> Result<Self>;

    fn name(&self) -> Result<String>;

    fn handle(&self, kind: String, payload: String) -> Result<String>;
}

fn plugin_root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/plugins"))
}

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut registry: Registry<HandlerClass> = Registry::new(Lua::new());
    let report = registry.load_dir(plugin_root())?;

    // `counter` depends on `formatter`, so it loads after it. `broken` raises while
    // loading and is reported rather than aborting the others.
    println!("loaded  : {:?}", report.loaded);
    for failure in &report.failures {
        println!("failed  : {} — {}", failure.name, first_line(&failure.reason.to_string()));
    }

    println!("\n-- dispatching an event to every plugin --");
    for outcome in registry.dispatch(|plugin| plugin.handle("deploy".into(), "v1.4.2".into())) {
        match outcome.result {
            Ok(text) => println!("  {:<10} {text}", outcome.name),
            // `grumpy` fails every call; the others still ran.
            Err(err) => {
                println!("  {:<10} failed: {}", outcome.name, first_line(&err.to_string()));
            }
        }
    }

    println!("\n-- dispatching again: `counter` keeps its state --");
    for outcome in registry.dispatch(|plugin| plugin.handle("rollback".into(), "v1.4.1".into())) {
        if let Ok(text) = outcome.result {
            println!("  {:<10} {text}", outcome.name);
        }
    }

    // `counter` sees only what `formatter` published. The handle dependents receive
    // is a proxy with no keys of its own, so probe it rather than iterating it.
    if let Some(formatter) = registry.get("formatter")
        && let Some(exports) = formatter.exports()
    {
        let published = exports.get::<Option<stanchion_lua::mlua::Function>>("decorate")?;
        let private = exports.get::<Option<stanchion_lua::mlua::Value>>("handle")?;
        println!("\nformatter exports `decorate`: {}", published.is_some());
        println!("formatter exports `handle`  : {}", private.is_some());
    }

    Ok(())
}

/// Lua errors carry a traceback; an example reads better with just the message.
fn first_line(message: &str) -> &str {
    message.lines().next().unwrap_or(message)
}

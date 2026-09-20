//! Running plugins you did not write: a sandbox, resource limits, and capabilities
//! that the host narrows rather than rubber-stamps.
//!
//! ```sh
//! cargo run -p stanchion --features lua54,vendored,registry --example untrusted
//! ```

use std::path::PathBuf;

use stanchion::lua_class;
use stanchion::mlua::{Lua, Result, Table, Value};
use stanchion::registry::{toml, CapabilityRequest, Decision, Registry, Rules, Sandbox};

#[lua_class]
pub trait Task {
    fn new(config: Table, deps: Table) -> Result<Self>;
    fn run(&self) -> Result<String>;
}

fn plugin_root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/untrusted"))
}

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut registry: Registry<TaskClass> = Registry::isolated(
        Lua::new(),
        // No `io`, no `os`, and a ceiling on both memory and per-call work.
        Sandbox::restricted()
            .memory_limit(8 * 1024 * 1024)
            .instruction_limit(200_000),
    )
    .with_setup(|host| {
        // The host offers `kv`; the policy below decides who actually gets it.
        host.capability("kv", |lua, grant| {
            // The approved namespace is baked into the closure, so a plugin cannot
            // widen it at call time — there is no parameter left to tamper with.
            let namespace: String = grant.get_or_default("namespace");
            Ok(Value::Function(lua.create_function(move |_, key: String| {
                Ok(format!("{namespace}/{key}"))
            })?))
        });
        Ok(())
    })
    .with_policy(Rules::deny_all().allow_with("kv", |request: &CapabilityRequest| {
        // The plugin asked for `tenant-7`. The host grants a namespace of its own
        // choosing instead: a policy that can only say yes or no is a rubber stamp.
        let mut narrowed = toml::Table::new();
        narrowed.insert(
            "namespace".to_string(),
            toml::Value::String(format!("sandboxed/{}", request.plugin)),
        );
        Decision::GrantWith(narrowed)
    }));

    let report = registry.load_dir(plugin_root())?;
    println!("loaded: {:?}\n", report.loaded);

    for plugin in registry.plugins() {
        print!("{:<8} ", plugin.name());
        match plugin.instance().run() {
            Ok(text) => println!("{text}"),
            // `looper` spins forever; the instruction limit turns that into an error
            // instead of a hung process.
            Err(err) => println!("stopped: {}", err.to_string().lines().next().unwrap_or("")),
        }
    }

    println!("\ngranted to `greedy`: {:?}", registry
        .get("greedy")
        .map(|plugin| plugin.granted_capabilities().collect::<Vec<_>>()));

    // A capability can be taken back from a running plugin.
    registry.revoke("greedy", "kv")?;
    print!("after revoking kv:   ");
    match registry.get("greedy").ok_or("greedy should be loaded")?.instance().run() {
        Ok(text) => println!("{text}"),
        Err(err) => println!("{}", err.to_string().lines().next().unwrap_or("")),
    }

    // Static review: what every plugin asks for, without running any of its code.
    println!("\n-- audit (nothing executed) --");
    for entry in registry.audit(plugin_root())?.plugins {
        let wants: Vec<&str> = entry.requests.iter().map(|r| r.name.as_str()).collect();
        println!("  {:<8} requests {wants:?}", entry.name);
    }

    Ok(())
}

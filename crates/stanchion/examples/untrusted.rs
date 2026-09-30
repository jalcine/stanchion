//! Running plugins you did not write: a sandbox, resource limits, and capabilities
//! that the host narrows rather than rubber-stamps.
//!
//! ```sh
//! cargo run -p stanchion --features lua54,vendored,registry --example untrusted
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;

use stanchion::registry::{
    CapabilityCall, CapabilityRequest, Decision, Registry, Rules, Value,
};
use stanchion_lua::backend::LuaBackend;
use stanchion_lua::sandbox::Sandbox;

fn plugin_root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/untrusted"))
}

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut registry = Registry::new()
        .with_runtime(Box::new(
            LuaBackend::isolated(
                // No `io`, no `os`, and a ceiling on both memory and per-call work.
                Sandbox::restricted()
                    .memory_limit(8 * 1024 * 1024)
                    .instruction_limit(200_000),
            ),
        ))
        .with_setup(|host| {
            // The host offers `kv`; the policy below decides who actually gets it.
            host.capability("kv", |call: &CapabilityCall| {
                // The approved namespace travels in the grant, so a plugin cannot
                // widen it at call time — there is no parameter left to tamper with.
                let namespace: String = call.grant.get_or_default("namespace");
                let key = match call.args.first() {
                    Some(Value::Str(key)) => key.clone(),
                    _ => return Err("expected a key string".to_string()),
                };
                Ok(Value::Str(format!("{namespace}/{key}")))
            });
            Ok(())
        })
        .with_policy(
            Rules::deny_all().allow_with("kv", |request: &CapabilityRequest| {
                // The plugin asked for `tenant-7`. The host grants a namespace of its own
                // choosing instead: a policy that can only say yes or no is a rubber stamp.
                let mut narrowed = BTreeMap::new();
                narrowed.insert(
                    "namespace".to_string(),
                    Value::Str(format!("sandboxed/{}", request.plugin)),
                );
                Decision::GrantWith(Value::Map(narrowed))
            }),
        );

    let report = registry.load_dir(plugin_root())?;
    println!("loaded: {:?}\n", report.loaded);

    for plugin in registry.plugins() {
        print!("{:<8} ", plugin.name());
        match registry.call(plugin.name(), "run", &[]) {
            Ok(Value::Str(text)) => println!("{text}"),
            Ok(other) => println!("{other:?}"),
            // `looper` spins forever; the instruction limit turns that into an error
            // instead of a hung process.
            Err(err) => println!("stopped: {}", err.to_string().lines().next().unwrap_or("")),
        }
    }

    println!(
        "\ngranted to `greedy`: {:?}",
        registry
            .get("greedy")
            .map(|plugin| plugin.granted_capabilities().collect::<Vec<_>>())
    );

    // A capability can be taken back from a running plugin.
    registry.revoke("greedy", "kv")?;
    print!("after revoking kv:   ");
    match registry.call("greedy", "run", &[]) {
        Ok(Value::Str(text)) => println!("{text}"),
        Ok(other) => println!("{other:?}"),
        Err(err) => println!("{}", err.to_string().lines().next().unwrap_or("")),
    }

    // Static review: what every plugin asks for, without running any of its code.
    println!("\n-- audit (nothing executed) --");
    for entry in registry.audit(plugin_root())?.plugins {
        let wants: Vec<&str> = entry
            .requests
            .iter()
            .map(|r| r.capability.as_str())
            .collect();
        println!("  {:<8} requests {wants:?}", entry.name);
    }

    Ok(())
}

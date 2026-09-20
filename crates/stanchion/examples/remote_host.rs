//! Plugins in a separate process, over JSON-RPC 2.0.
//!
//! What this buys over in-process hosting: a plugin that kills the interpreter kills
//! the child, not you. The last section proves it.
//!
//! ```sh
//! cargo build -p stanchion --features lua54,vendored,remote --bin plugin-host
//! cargo run   -p stanchion --features lua54,vendored,remote --example remote_host
//! ```

use std::path::PathBuf;

use stanchion::remote::{CallbackCall, RemoteOptions, RemoteRegistry};
use serde_json::{json, Value as Json};

fn example_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/remote"))
}

/// Examples do not get `CARGO_BIN_EXE_*`, so find the binary beside this one.
fn host_binary() -> std::result::Result<PathBuf, Box<dyn std::error::Error>> {
    let mut dir = std::env::current_exe()?;
    dir.pop();
    if dir.ends_with("examples") {
        dir.pop();
    }
    let path = dir.join("plugin-host");
    if !path.is_file() {
        return Err(format!(
            "`plugin-host` not found at {}; build it with\n  \
             cargo build -p stanchion --features lua54,vendored,remote --bin plugin-host",
            path.display()
        )
        .into());
    }
    Ok(path)
}

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let options = RemoteOptions::new(host_binary()?)
        .config(example_dir().join("host.toml"))
        .plugins(example_dir());

    // The application answers the capabilities its plugins call. The grant that the
    // host's policy approved travels with every call, so this can re-check it rather
    // than trusting the host to have narrowed correctly.
    let mut remote = RemoteRegistry::launch(options)?.on_callback(|call: &CallbackCall| {
        let allowed: Vec<&str> = call
            .grant
            .get("hosts")
            .and_then(Json::as_array)
            .map(|hosts| hosts.iter().filter_map(Json::as_str).collect())
            .unwrap_or_default();

        let url = call.args.first().and_then(Json::as_str).unwrap_or("");
        match allowed.iter().find(|host| url.contains(*host)) {
            Some(host) => Ok(json!(format!("200 OK from {host}"))),
            None => Err(format!("`{url}` is not in this plugin's grant")),
        }
    });

    for plugin in remote.list()? {
        println!("loaded: {} {:?} granted {:?}", plugin.name, plugin.version, plugin.granted);
    }

    println!("\n-- a plugin calling back into this process --");
    let allowed: String = remote.call("fetcher", "fetch", [json!("https://example.test/a")])?;
    println!("  allowed host : {allowed}");

    match remote.call::<Json>("fetcher", "fetch", [json!("https://evil.test/a")]) {
        Ok(value) => println!("  unexpected   : {value}"),
        Err(err) => println!("  refused host : {}", first_line(&err.to_string())),
    }

    println!("\n-- the point of a separate process --");
    match remote.call::<Json>("fetcher", "crash", []) {
        Ok(_) => println!("  the plugin was supposed to kill its process"),
        // In-process there is no recovering from this; here it is one error value.
        Err(err) => println!("  plugin killed the host: {err}"),
    }
    println!("  host alive   : {}", remote.is_alive());
    println!("  this process : still running");

    Ok(())
}

/// Lua errors carry a traceback; an example reads better with just the message.
fn first_line(message: &str) -> &str {
    message.lines().next().unwrap_or(message)
}

//! Out-of-process plugin host with Lua backend.
//!
//! ```text
//! plugin-host-lua --config host.toml [--plugins DIR]
//! ```
//!
//! stdin and stdout carry JSON-RPC 2.0, one message per line; **stderr is free for
//! logging**, which is why the built-in `log` capability writes there.

use std::process::ExitCode;

const BIN: &str = "plugin-host-lua";

const USAGE: &str = "\
plugin-host-lua — run Lua plugins in an isolated process

USAGE:
    plugin-host-lua [--config FILE] [--plugins DIR]

OPTIONS:
    --config FILE   TOML host configuration (sandbox, capabilities, signatures)
    --plugins DIR   Plugin root to load at startup, overriding the config
    -h, --help      Print this message
";

fn main() -> ExitCode {
    match stanchion_remote::bin_common::run(BIN, USAGE) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{BIN}: {message}");
            ExitCode::FAILURE
        }
    }
}

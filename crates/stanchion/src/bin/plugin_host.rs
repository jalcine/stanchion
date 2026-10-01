//! Out-of-process plugin host.
//!
//! Runs plugins in a child process and speaks length-prefixed JSON over stdio, so a
//! plugin that loops forever, exhausts memory, or crashes the interpreter takes down
//! this process instead of the application that launched it.
//!
//! ```text
//! plugin-host --config host.toml [--plugins DIR]
//! ```
//!
//! stdin and stdout carry JSON-RPC 2.0, one message per line; **stderr is free for
//! logging**, which is why the built-in `log` capability writes there.

use std::process::ExitCode;

const BIN: &str = "plugin-host";

const USAGE: &str = "\
plugin-host — run Lua plugins in an isolated process

USAGE:
    plugin-host [--config FILE] [--plugins DIR]

OPTIONS:
    --config FILE   TOML host configuration (sandbox, capabilities, signatures)
    --plugins DIR   Plugin root to load at startup, overriding the config
    -h, --help      Print this message

The protocol is length-prefixed JSON on stdin/stdout. stderr is for logs.";

fn main() -> ExitCode {
    match stanchion::remote::bin_common::run(BIN, USAGE) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{BIN}: {message}");
            ExitCode::FAILURE
        }
    }
}

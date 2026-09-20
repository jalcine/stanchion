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
//! stdin and stdout carry the protocol; **stderr is free for logging**, which is why
//! the built-in `log` capability writes there.

use std::io;
use std::path::PathBuf;
use std::process::ExitCode;

use stanchion_remote::{build_registry, load_config, serve};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("plugin-host: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let options = Options::parse(std::env::args().skip(1))?;
    if options.help {
        println!("{USAGE}");
        return Ok(());
    }

    let mut config = match &options.config {
        Some(path) => load_config(path)?,
        None => Default::default(),
    };
    if let Some(plugins) = options.plugins {
        config.plugins = Some(plugins);
    }

    let mut registry = build_registry(&config)?;

    // Loading up front keeps the client's first call fast, and surfaces a broken
    // plugin root before any request arrives.
    if let Some(root) = &config.plugins {
        let report = registry
            .load_dir(root)
            .map_err(|err| format!("loading `{}`: {err}", root.display()))?;
        for failure in &report.failures {
            eprintln!("plugin-host: {failure}");
        }
    }

    serve(&mut registry, io::stdin().lock(), io::stdout().lock())
        .map_err(|err| format!("serving: {err}"))
}

const USAGE: &str = "\
plugin-host — run Lua plugins in an isolated process

USAGE:
    plugin-host [--config FILE] [--plugins DIR]

OPTIONS:
    --config FILE   TOML host configuration (sandbox, capabilities, signatures)
    --plugins DIR   Plugin root to load at startup, overriding the config
    -h, --help      Print this message

The protocol is length-prefixed JSON on stdin/stdout. stderr is for logs.";

struct Options {
    config: Option<PathBuf>,
    plugins: Option<PathBuf>,
    help: bool,
}

impl Options {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut options = Options { config: None, plugins: None, help: false };
        let mut args = args.peekable();

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-h" | "--help" => options.help = true,
                "--config" => {
                    options.config =
                        Some(PathBuf::from(args.next().ok_or("--config needs a path")?));
                }
                "--plugins" => {
                    options.plugins =
                        Some(PathBuf::from(args.next().ok_or("--plugins needs a path")?));
                }
                other => return Err(format!("unexpected argument `{other}`\n\n{USAGE}")),
            }
        }
        Ok(options)
    }
}

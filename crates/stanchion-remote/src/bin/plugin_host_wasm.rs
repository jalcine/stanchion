//! Out-of-process plugin host with WASM backend.
//!
//! ```text
//! plugin-host-wasm --config host.toml [--plugins DIR]
//! ```
//!
//! stdin and stdout carry JSON-RPC 2.0, one message per line; **stderr is free for
//! logging**, which is why the built-in `log` capability writes there.

use std::io;
use std::path::PathBuf;
use std::process::ExitCode;

use std::io::BufReader;

use stanchion_remote::{HostChannel, build_registry, load_config, serve};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("plugin-host-wasm: {message}");
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

    let channel = HostChannel::new(BufReader::new(io::stdin()), io::BufWriter::new(io::stdout()));
    let mut registry = build_registry(&config, &channel)?;

    if let Some(root) = &config.plugins {
        let report = registry
            .load_dir(root)
            .map_err(|err| format!("loading `{}`: {err}", root.display()))?;
        for failure in &report.failures {
            eprintln!("plugin-host-wasm: {failure}");
        }
    }

    serve(&mut registry, &channel).map_err(|err| format!("serving: {err}"))
}

const USAGE: &str = "\
plugin-host-wasm — run WASM plugins in an isolated process

USAGE:
    plugin-host-wasm [--config FILE] [--plugins DIR]

OPTIONS:
    --config FILE   TOML host configuration (sandbox, capabilities, signatures)
    --plugins DIR   Plugin root to load at startup, overriding the config
    -h, --help      Print this message
";

struct Options {
    config: Option<PathBuf>,
    plugins: Option<PathBuf>,
    help: bool,
}

impl Options {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut options = Options {
            config: None,
            plugins: None,
            help: false,
        };
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

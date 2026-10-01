//! Shared entrypoint for the out-of-process plugin host binaries.
//!
//! The `plugin-host`, `plugin-host-lua` and `plugin-host-wasm` binaries were
//! three copies of the same `Options::parse` + `run` block, differing only in
//! the binary name in log lines and `USAGE`. One owner removes that drift: a
//! thin `main` passes its name and usage text, everything else lives here.

use std::io::BufReader;
use std::path::PathBuf;

use crate::{HostChannel, build_registry, load_config, serve};

/// Command-line options every host binary accepts.
pub struct Options {
    /// TOML host configuration file.
    pub config: Option<PathBuf>,
    /// Plugin root to load at startup, overriding the config.
    pub plugins: Option<PathBuf>,
    /// Whether `--help` was passed.
    pub help: bool,
}

impl Options {
    /// Parses `--config`, `--plugins` and `-h`/`--help`.
    pub fn parse(
        args: impl Iterator<Item = String>,
        usage: &str,
    ) -> Result<Self, String> {
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
                other => return Err(format!("unexpected argument `{other}`\n\n{usage}")),
            }
        }
        Ok(options)
    }
}

/// Runs a host binary end to end: parse args, build the registry, load the
/// configured plugin root up front, then serve until shutdown.
///
/// `binary` names the binary for log lines; `usage` is printed for `--help`
/// and for argument errors.
pub fn run(binary: &str, usage: &str) -> Result<(), String> {
    let options = Options::parse(std::env::args().skip(1), usage)?;
    if options.help {
        println!("{usage}");
        return Ok(());
    }

    let mut config = match &options.config {
        Some(path) => load_config(path)?,
        None => Default::default(),
    };
    if let Some(plugins) = options.plugins {
        config.plugins = Some(plugins);
    }

    let channel = HostChannel::new(
        BufReader::new(std::io::stdin()),
        std::io::BufWriter::new(std::io::stdout()),
    );
    let (mut registry, isolation) = build_registry(&config, &channel)?;

    // Loading up front keeps the client's first call fast, and surfaces a broken
    // plugin root before any request arrives.
    if let Some(root) = &config.plugins {
        let report = registry
            .load_dir(root)
            .map_err(|err| format!("loading `{}`: {err}", root.display()))?;
        for failure in &report.failures {
            eprintln!("{binary}: {failure}");
        }
    }

    serve(&mut registry, &channel, isolation).map_err(|err| format!("serving: {err}"))
}

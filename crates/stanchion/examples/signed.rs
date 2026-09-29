//! Provenance tiers authority: who signed decides what a plugin may reach.
//!
//! A stub verifier reads a `plugin.sig` identity file instead of a real bundle, so
//! this runs offline. `shipped` carries one and earns `kv`; `unsigned` carries
//! none, loads as `Signer::Unsigned`, and degrades to no capability because it
//! declared `kv` optional.
//!
//! ```sh
//! cargo run -p stanchion --features lua54,vendored,signatures --example signed
//! ```

use std::io;
use std::path::{Path, PathBuf};

use stanchion::registry::{
    CapabilityRequest, Decision, DirectoryDigest, HostSetup, PluginVerifier, Registry, Revocations,
    Rules, Sandbox, Signer, VerifyError,
};
use stanchion_lua::lua_class;
use stanchion_lua::mlua::{Lua, Result, Table, Value};

#[lua_class]
pub trait Task {
    fn new(config: Table, deps: Table) -> Result<Self>;
    fn run(&self) -> Result<String>;
}

/// Trusts whatever identity a `plugin.sig` file names.
///
/// A real host would verify a bundle against its trust root here; the registry only
/// decides what to do with the answer, so the shape is the same.
struct FileVerifier;

impl PluginVerifier for FileVerifier {
    fn verify(
        &self,
        _digest: &DirectoryDigest,
        dir: &Path,
    ) -> std::result::Result<Signer, VerifyError> {
        match std::fs::read_to_string(dir.join("plugin.sig")) {
            Ok(identity) => {
                let identity = identity.trim().to_string();
                if identity == "repo:acme/compromised" {
                    Err(VerifyError::Untrusted(format!("{identity} is not trusted")))
                } else {
                    Ok(Signer::verified(identity, None))
                }
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => Err(VerifyError::Missing),
            Err(err) => Err(VerifyError::Io(err)),
        }
    }
}

fn plugin_root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/signed"))
}

/// Everything the host offers: a `kv` capability whose approved namespace is baked
/// into the value, so a plugin cannot widen it at call time.
fn offer_kv(host: &mut HostSetup) -> Result<()> {
    host.capability("kv", |runtime, grant| {
        let namespace: String = grant.get_or_default("namespace");
        let state = runtime
            .lua_state()
            .ok_or_else(|| stanchion_abi::Error::Config("`kv` needs a Lua runtime".into()))?;
        let lua = state
            .lock()
            .map_err(|_| stanchion_abi::Error::Config("the Lua state is poisoned".into()))?;
        let kv = lua
            .create_function(move |_, key: String| Ok(format!("{namespace}/{key}")))
            .map_err(|err| stanchion_abi::Error::Config(err.to_string()))?;
        Ok(stanchion_abi::value::lua::lua_to_abi(
            &lua,
            &Value::Function(kv),
        ))
    });
    Ok(())
}

/// First-party signatures earn `kv`; anything else is refused the capability rather
/// than refused the load.
fn first_party_kv() -> Rules {
    Rules::deny_all().allow_with("kv", |request: &CapabilityRequest| {
        match request.signer().identity() {
            Some(identity) if identity.starts_with("repo:acme/") => Decision::Grant,
            Some(identity) => Decision::deny(format!(
                "`kv` needs a first-party signature, not {identity}"
            )),
            None => Decision::deny("`kv` needs a first-party signature"),
        }
    })
}

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut registry: Registry<TaskClass> = Registry::isolated(Lua::new(), Sandbox::restricted())
        .with_setup(offer_kv)
        .with_policy(first_party_kv())
        .with_verifier(FileVerifier);

    let report = registry.load_dir(plugin_root())?;
    println!("loaded: {:?}\n", report.loaded);
    for failure in &report.failures {
        println!(
            "failed: {} — {}",
            failure.name,
            first_line(&failure.reason.to_string())
        );
    }

    for plugin in registry.plugins() {
        print!(
            "{:<8} signer={:<18} ",
            plugin.name(),
            plugin.signer().to_string()
        );
        match plugin.instance().run() {
            Ok(text) => println!("{text}"),
            Err(err) => println!("stopped: {}", first_line(&err.to_string())),
        }
    }

    // The digest is what a signature covers and what a lockfile pins.
    let digest = DirectoryDigest::compute(&plugin_root().join("shipped"))?;
    println!("\nshipped digest: sha256:{}", digest.hex());

    // Requiring signatures turns the gradient into a cliff: the unsigned plugin is
    // refused outright instead of loading degraded.
    let mut strict: Registry<TaskClass> = Registry::isolated(Lua::new(), Sandbox::restricted())
        .with_setup(offer_kv)
        .with_policy(first_party_kv())
        .with_verifier(FileVerifier)
        .require_signatures(true);
    let report = strict.load_dir(plugin_root())?;
    println!("\nstrict loaded: {:?}", report.loaded);
    for failure in &report.failures {
        println!(
            "strict refused: {} — {}",
            failure.name,
            first_line(&failure.reason.to_string())
        );
    }

    // Revocation is the mutable half of provenance: the signature stays valid, the
    // host's decision changes, and the running plugin is unloaded.
    let refused = registry
        .apply_revocations(Revocations::new().deny_identity("repo:acme/plugins", "key compromise"));
    for failure in &refused {
        println!(
            "\nunloaded: {} — {}",
            failure.name,
            first_line(&failure.reason.to_string())
        );
    }
    println!(
        "still loaded: {:?}",
        registry
            .plugins()
            .iter()
            .map(|p| p.name())
            .collect::<Vec<_>>()
    );

    Ok(())
}

fn first_line(message: &str) -> &str {
    message.lines().next().unwrap_or(message)
}

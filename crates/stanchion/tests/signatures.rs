//! Signature verification: what is signed, when it is checked, and how provenance
//! feeds capability policy.
//!
//! These use a stub verifier so the wiring is covered without a crypto dependency.
#![cfg(feature = "signatures")]

use std::fs;
use std::path::Path;
use std::sync::Mutex;

use stanchion::lua_class;
use stanchion::mlua::{Lua, Result, Table, Value};
use stanchion::registry::{
    Decision, DirectoryDigest, FailureReason, PluginVerifier, Registry, Rules, Sandbox, Signer,
    VerifyError, SIGNATURE_FILE,
};
use tempfile::TempDir;

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;
type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[lua_class]
pub trait Probe {
    fn new(config: Table, deps: Table) -> Result<Self>;
    fn run(&self, input: String) -> Result<String>;
}

/// Trusts a `plugin.sig` whose contents are the expected root digest in hex, and
/// records every digest it was asked about.
struct StubVerifier {
    identity: String,
    seen: Mutex<Vec<String>>,
}

impl StubVerifier {
    fn new(identity: &str) -> Self {
        StubVerifier { identity: identity.to_string(), seen: Mutex::new(Vec::new()) }
    }
}

impl PluginVerifier for StubVerifier {
    fn verify(&self, digest: &DirectoryDigest, dir: &Path) -> std::result::Result<Signer, VerifyError> {
        if let Ok(mut seen) = self.seen.lock() {
            seen.push(digest.hex());
        }
        let path = dir.join(SIGNATURE_FILE);
        let claimed = match fs::read_to_string(&path) {
            Ok(claimed) => claimed.trim().to_string(),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err(VerifyError::Missing);
            }
            Err(err) => return Err(VerifyError::Io(err)),
        };
        if claimed == digest.hex() {
            Ok(Signer::verified(self.identity.clone(), Some("stub".to_string())))
        } else {
            Err(VerifyError::Invalid("digest does not match the signature".to_string()))
        }
    }
}

fn probe_source(body: &str) -> String {
    format!(
        "local P = {{}}\nP.__index = P\n\
         function P.new(config, deps) return setmetatable({{}}, P) end\n\
         function P:run(input)\n  {body}\nend\nreturn P\n"
    )
}

fn write_plugin(root: &Path, name: &str, manifest: &str, source: &str) -> TestResult {
    let dir = root.join(name);
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("plugin.toml"), manifest)?;
    fs::write(dir.join("init.lua"), source)?;
    Ok(())
}

/// Signs a plugin directory by writing its current digest into `plugin.sig`.
fn sign(dir: &Path) -> TestResult {
    let digest = DirectoryDigest::compute(dir)?;
    fs::write(dir.join(SIGNATURE_FILE), digest.hex())?;
    Ok(())
}

fn one_plugin(manifest: &str, body: &str) -> Fallible<TempDir> {
    let root = tempfile::tempdir()?;
    write_plugin(root.path(), "probe", manifest, &probe_source(body))?;
    Ok(root)
}

fn first_failure(report: &stanchion::registry::LoadReport) -> Fallible<&FailureReason> {
    report
        .failures
        .first()
        .map(|failure| &failure.reason)
        .ok_or_else(|| "expected the plugin to fail".into())
}

#[test]
fn the_digest_covers_every_file_not_just_the_manifest() -> TestResult {
    let root = one_plugin("name = \"probe\"\n", r#"return "a""#)?;
    let dir = root.path().join("probe");
    let before = DirectoryDigest::compute(&dir)?;

    // Swapping the code while leaving the manifest alone must change the digest,
    // or a signature would attest to capabilities the code does not match.
    fs::write(dir.join("init.lua"), probe_source(r#"return "b""#))?;
    let after = DirectoryDigest::compute(&dir)?;
    assert_ne!(before.hex(), after.hex());

    // A new file counts too.
    fs::write(dir.join("extra.lua"), "return {}\n")?;
    assert_ne!(after.hex(), DirectoryDigest::compute(&dir)?.hex());
    Ok(())
}

#[test]
fn the_digest_ignores_the_signature_artifact_itself() -> TestResult {
    let root = one_plugin("name = \"probe\"\n", r#"return "a""#)?;
    let dir = root.path().join("probe");
    let before = DirectoryDigest::compute(&dir)?;

    fs::write(dir.join(SIGNATURE_FILE), "anything at all")?;
    assert_eq!(before.hex(), DirectoryDigest::compute(&dir)?.hex());
    Ok(())
}

#[test]
fn the_digest_is_reproducible_and_covers_nested_files() -> TestResult {
    let root = one_plugin("name = \"probe\"\n", r#"return "a""#)?;
    let dir = root.path().join("probe");
    fs::create_dir_all(dir.join("lib"))?;
    fs::write(dir.join("lib").join("helper.lua"), "return {}\n")?;

    let first = DirectoryDigest::compute(&dir)?;
    let second = DirectoryDigest::compute(&dir)?;
    assert_eq!(first, second);
    assert_eq!(first.len(), 3, "manifest, entry and nested helper");
    assert!(first.covers("lib/helper.lua"));
    Ok(())
}

#[test]
fn a_valid_signature_yields_a_verified_signer() -> TestResult {
    let root = one_plugin("name = \"probe\"\n", r#"return "ok""#)?;
    sign(&root.path().join("probe"))?;

    let mut registry: Registry<ProbeClass> = Registry::isolated(Lua::new(), Sandbox::restricted())
        .with_verifier(StubVerifier::new("repo:acme/plugins"))
        .require_signatures(true);
    let report = registry.load_dir(root.path())?;
    assert!(report.is_clean(), "failures: {:?}", report.failures);

    let probe = registry.get("probe").ok_or("probe should load")?;
    assert!(probe.signer().is_verified());
    assert_eq!(probe.signer().identity(), Some("repo:acme/plugins"));
    assert_eq!(probe.signer().issuer(), Some("stub"));
    Ok(())
}

#[test]
fn tampering_after_signing_is_caught() -> TestResult {
    let root = one_plugin("name = \"probe\"\n", r#"return "ok""#)?;
    let dir = root.path().join("probe");
    sign(&dir)?;
    // Same manifest, different code.
    fs::write(dir.join("init.lua"), probe_source(r#"return "tampered""#))?;

    let mut registry: Registry<ProbeClass> = Registry::isolated(Lua::new(), Sandbox::restricted())
        .with_verifier(StubVerifier::new("repo:acme/plugins"));
    let report = registry.load_dir(root.path())?;

    assert!(report.loaded.is_empty());
    assert!(
        matches!(first_failure(&report)?, FailureReason::SignatureInvalid(_)),
        "got: {:?}",
        report.failures
    );
    Ok(())
}

#[test]
fn require_signatures_rejects_an_unsigned_plugin() -> TestResult {
    let root = one_plugin("name = \"probe\"\n", r#"return "ok""#)?;

    let mut registry: Registry<ProbeClass> = Registry::isolated(Lua::new(), Sandbox::restricted())
        .with_verifier(StubVerifier::new("repo:acme/plugins"))
        .require_signatures(true);
    let report = registry.load_dir(root.path())?;

    assert!(matches!(first_failure(&report)?, FailureReason::Unsigned), "{:?}", report.failures);
    Ok(())
}

#[test]
fn without_require_signatures_an_unsigned_plugin_loads_as_unsigned() -> TestResult {
    let root = one_plugin("name = \"probe\"\n", r#"return "ok""#)?;

    let mut registry: Registry<ProbeClass> = Registry::isolated(Lua::new(), Sandbox::restricted())
        .with_verifier(StubVerifier::new("repo:acme/plugins"));
    let report = registry.load_dir(root.path())?;
    assert!(report.is_clean(), "failures: {:?}", report.failures);

    let probe = registry.get("probe").ok_or("probe should load")?;
    assert_eq!(probe.signer(), &Signer::Unsigned);
    Ok(())
}

#[test]
fn provenance_tiers_capability_grants() -> TestResult {
    // The same policy, the same manifest: only the signature differs.
    let manifest = "name = \"probe\"\n\n[capabilities.network]\noptional = true\n";
    let body = r#"return type(network)"#;

    let build = |signed: bool| -> Fallible<String> {
        let root = one_plugin(manifest, body)?;
        if signed {
            sign(&root.path().join("probe"))?;
        }
        let mut registry: Registry<ProbeClass> =
            Registry::isolated(Lua::new(), Sandbox::restricted())
                .with_verifier(StubVerifier::new("repo:acme/plugins"))
                .with_setup(|host| {
                    host.capability("network", |lua, _grant| {
                        Ok(Value::Function(lua.create_function(|_, ()| Ok(()))?))
                    });
                    Ok(())
                })
                .with_policy(Rules::deny_all().allow_with("network", |request| {
                    match request.signer().identity() {
                        Some(identity) if identity.starts_with("repo:acme/") => Decision::Grant,
                        _ => Decision::deny("network requires a first-party signature"),
                    }
                }));
        registry.load_dir(root.path())?;
        let probe = registry.get("probe").ok_or("probe should load")?;
        Ok(probe.instance().run(String::new())?)
    };

    assert_eq!(build(true)?, "function", "a signed plugin earns the capability");
    assert_eq!(build(false)?, "nil", "an unsigned one does not");
    Ok(())
}

#[test]
fn an_untrusted_signer_is_distinguished_from_a_bad_signature() -> TestResult {
    struct AlwaysUntrusted;
    impl PluginVerifier for AlwaysUntrusted {
        fn verify(
            &self,
            _digest: &DirectoryDigest,
            _dir: &Path,
        ) -> std::result::Result<Signer, VerifyError> {
            Err(VerifyError::Untrusted("unknown key".to_string()))
        }
    }

    let root = one_plugin("name = \"probe\"\n", r#"return "ok""#)?;
    let mut registry: Registry<ProbeClass> =
        Registry::isolated(Lua::new(), Sandbox::restricted()).with_verifier(AlwaysUntrusted);
    let report = registry.load_dir(root.path())?;

    assert!(
        matches!(first_failure(&report)?, FailureReason::UntrustedSigner(_)),
        "got: {:?}",
        report.failures
    );
    Ok(())
}

#[test]
fn a_submodule_altered_after_verification_is_refused() -> TestResult {
    // The digest is taken at discovery; `require` re-checks as it reads, so a file
    // swapped in between does not get to run.
    let root = tempfile::tempdir()?;
    // Requiring at chunk scope means the check fires during load, not on first call.
    write_plugin(
        root.path(),
        "probe",
        "name = \"probe\"\n",
        "local helper = require(\"helper\")\n         local P = {}\nP.__index = P\n         function P.new(config, deps) return setmetatable({}, P) end\n         function P:run(input) return helper.value() end\n         return P\n",
    )?;
    let dir = root.path().join("probe");
    fs::write(dir.join("helper.lua"), "return { value = function() return \"clean\" end }\n")?;

    /// Verifies the clean digest, then swaps a file — exactly the race between
    /// checking and loading that per-file hashes exist to close.
    struct TamperingVerifier {
        target: std::path::PathBuf,
    }

    impl PluginVerifier for TamperingVerifier {
        fn verify(
            &self,
            digest: &DirectoryDigest,
            _dir: &Path,
        ) -> std::result::Result<Signer, VerifyError> {
            assert!(digest.covers("helper.lua"));
            fs::write(&self.target, "return { value = function() return \"evil\" end }\n")?;
            Ok(Signer::verified("test", None))
        }
    }

    let mut registry: Registry<ProbeClass> = Registry::isolated(
        Lua::new(),
        Sandbox::restricted(),
    )
    .with_verifier(TamperingVerifier { target: dir.join("helper.lua") });

    let report = registry.load_dir(root.path())?;

    assert!(report.loaded.is_empty(), "the tampered submodule must not load");
    let reason = first_failure(&report)?.to_string();
    assert!(
        reason.contains("helper.lua") && reason.contains("changed between"),
        "got: {reason}"
    );
    Ok(())
}

#[test]
fn audit_reports_provenance_without_running_code() -> TestResult {
    let root = tempfile::tempdir()?;
    write_plugin(
        root.path(),
        "signed",
        "name = \"signed\"\n",
        &probe_source(r#"return "x""#),
    )?;
    write_plugin(
        root.path(),
        "bare",
        "name = \"bare\"\n",
        "error('audit must not execute plugin code')\n",
    )?;
    sign(&root.path().join("signed"))?;

    let registry: Registry<ProbeClass> = Registry::isolated(Lua::new(), Sandbox::restricted())
        .with_verifier(StubVerifier::new("repo:acme/plugins"));
    let audit = registry.audit(root.path())?;

    let signer_of = |name: &str| {
        audit
            .plugins
            .iter()
            .find(|plugin| plugin.name == name)
            .map(|plugin| plugin.signer.clone())
    };
    assert_eq!(signer_of("bare"), Some(Signer::Unsigned));
    assert!(signer_of("signed").is_some_and(|signer| signer.is_verified()));
    Ok(())
}

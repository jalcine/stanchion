//! Distribution: packaging, an index, pinned installs, and upgrade review.
#![cfg(all(feature = "distribution", feature = "signatures"))]

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use mlua::{Lua, Result as LuaResult};
use stanchion::dist::index::{DirectoryIndex, Freshness, PluginReleases, Release};
use stanchion::dist::install::{PluginSource, SourceError};
use stanchion::dist::package::{self, Limits};
use stanchion::dist::{IndexError, Installer, PluginIndex};
use stanchion::lua_class;
use stanchion::registry::{DirectoryDigest, LockError, Lockfile, Registry, Sandbox};

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;
type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[lua_class]
pub trait Greeter {
    fn new(config: mlua::Table, deps: mlua::Table) -> LuaResult<Self>;
    fn greet(&self) -> LuaResult<String>;
}

/// Unwraps the error from something that was supposed to fail.
///
/// Returns the error itself rather than its message, so a caller can either match on
/// the variant or render it, whichever the case is actually about.
fn must_fail<T, E>(result: std::result::Result<T, E>, what: &str) -> Fallible<E> {
    match result {
        Err(err) => Ok(err),
        Ok(_) => Err(format!("expected {what} to fail, but it succeeded").into()),
    }
}

/// A plugin directory, written from scratch so each test states its own fixture.
fn write_plugin(dir: &Path, manifest: &str, body: &str) -> TestResult {
    fs::create_dir_all(dir)?;
    fs::write(dir.join("plugin.toml"), manifest)?;
    fs::write(dir.join("init.lua"), body)?;
    Ok(())
}

const GREETER: &str = r#"
local Greeter = {}
Greeter.__index = Greeter
function Greeter.new(config, deps)
  return setmetatable({ greeting = config.greeting or "hello" }, Greeter)
end
function Greeter:greet() return self.greeting end
return Greeter
"#;

/// A source backed by a map, so a test can hand over exactly the bytes it wants —
/// including bytes that are not the ones the index promised.
#[derive(Default)]
struct FakeSource {
    packages: HashMap<String, Vec<u8>>,
}

impl FakeSource {
    fn insert(&mut self, reference: &str, archive: Vec<u8>) {
        self.packages.insert(reference.to_string(), archive);
    }
}

impl PluginSource for FakeSource {
    fn fetch(&self, reference: &str) -> Result<Vec<u8>, SourceError> {
        self.packages
            .get(reference)
            .cloned()
            .ok_or_else(|| SourceError::NotFound(reference.to_string()))
    }
}

/// Packs a plugin directory and returns the archive with its directory digest.
fn pack(dir: &Path) -> Fallible<(Vec<u8>, DirectoryDigest)> {
    let mut archive = Vec::new();
    let digest = package::pack(dir, &mut archive)?;
    Ok((archive, digest))
}

/// Writes a one-plugin index under `base`.
fn write_index(base: &Path, releases: &PluginReleases) -> TestResult {
    let dir = base.join("v1/plugins");
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", releases.name));
    let raw = serde_json::to_string_pretty(releases)?;
    fs::write(path, raw)?;
    Ok(())
}

/// One release of one plugin, sourced from `reference`.
fn one_release(
    name: &str,
    version: &str,
    digest: &DirectoryDigest,
    reference: &str,
) -> Fallible<PluginReleases> {
    Ok(PluginReleases {
        schema: 1,
        name: name.to_string(),
        updated: None,
        expires: None,
        releases: vec![
            Release::new(version.parse()?, format!("sha256:{}", digest.hex()))
                .from_source(reference),
        ],
    })
}

#[test]
fn packing_round_trips_the_directory_digest() -> TestResult {
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("src/greeter");
    write_plugin(
        &source,
        "name = \"greeter\"\nversion = \"1.0.0\"\n",
        GREETER,
    )?;
    // A nested file, since the digest is over relative paths and folding order.
    fs::create_dir_all(source.join("lib"))?;
    fs::write(source.join("lib/util.lua"), "return {}")?;

    let (archive, digest) = pack(&source)?;

    let out = temp.path().join("out");
    fs::create_dir_all(&out)?;
    package::unpack(archive.as_slice(), &out, Limits::default())?;

    let round_tripped = DirectoryDigest::compute(&out)?;
    assert_eq!(
        digest.hex(),
        round_tripped.hex(),
        "the digest is over the directory, so it must survive a repack"
    );
    assert_eq!(round_tripped.len(), 3);
    Ok(())
}

/// Builds a one-entry `tar.gz` with the path written straight into the header.
///
/// The `tar` crate refuses to *write* a `..` path, which is the right default and
/// also the reason this has to bypass it: an archive arriving from a registry was not
/// necessarily produced by a well-behaved packer, so the unpacker cannot assume one.
fn forged_archive(name: &str, entry_type: tar::EntryType, link: Option<&str>) -> Fallible<Vec<u8>> {
    let mut header = tar::Header::new_gnu();
    header.set_size(0);
    header.set_mode(0o644);
    header.set_entry_type(entry_type);
    {
        let gnu = header.as_gnu_mut().ok_or("a GNU header")?;
        for (slot, byte) in gnu.name.iter_mut().zip(name.bytes()) {
            *slot = byte;
        }
        if let Some(target) = link {
            for (slot, byte) in gnu.linkname.iter_mut().zip(target.bytes()) {
                *slot = byte;
            }
        }
    }
    header.set_cksum();

    let mut raw = Vec::new();
    {
        let encoder = flate2::write::GzEncoder::new(&mut raw, flate2::Compression::fast());
        let mut builder = tar::Builder::new(encoder);
        builder.append(&header, std::io::empty())?;
        builder.into_inner()?.finish()?;
    }
    Ok(raw)
}

#[test]
fn unpacking_refuses_path_traversal_and_symlinks() -> TestResult {
    let temp = tempfile::tempdir()?;

    let cases = [
        ("../escape.lua", tar::EntryType::Regular, None),
        ("/absolute.lua", tar::EntryType::Regular, None),
        ("nested/../../escape.lua", tar::EntryType::Regular, None),
        ("link.lua", tar::EntryType::Symlink, Some("/etc/passwd")),
        ("hard.lua", tar::EntryType::Link, Some("/etc/passwd")),
    ];

    for (index, (name, entry_type, link)) in cases.into_iter().enumerate() {
        let raw = forged_archive(name, entry_type, link)?;
        let out = temp.path().join(format!("out-{index}"));
        fs::create_dir_all(&out)?;

        let result = package::unpack(raw.as_slice(), &out, Limits::default());
        assert!(
            result.is_err(),
            "`{name}` should be refused rather than written"
        );
        assert!(
            fs::read_dir(&out)?.next().is_none(),
            "`{name}` left something behind in the destination"
        );
    }

    assert!(
        !temp.path().join("escape.lua").exists(),
        "nothing may be written outside the destination"
    );
    Ok(())
}

#[test]
fn a_tampered_package_fails_the_digest_check() -> TestResult {
    let temp = tempfile::tempdir()?;
    let honest = temp.path().join("src/greeter");
    write_plugin(
        &honest,
        "name = \"greeter\"\nversion = \"1.0.0\"\n",
        GREETER,
    )?;
    let (_, digest) = pack(&honest)?;

    // Same manifest, different code — the attack a signature over `plugin.toml`
    // alone would miss entirely.
    let swapped = temp.path().join("swapped/greeter");
    write_plugin(
        &swapped,
        "name = \"greeter\"\nversion = \"1.0.0\"\n",
        GREETER.replace("self.greeting", "\"pwned\"").as_str(),
    )?;
    let (tampered_archive, _) = pack(&swapped)?;

    let index_base = temp.path().join("index");
    write_index(
        &index_base,
        &one_release("greeter", "1.0.0", &digest, "fake://greeter:1.0.0")?,
    )?;

    let mut source = FakeSource::default();
    source.insert("fake://greeter:1.0.0", tampered_archive);

    let root = temp.path().join("plugins");
    let installer = Installer::new(
        DirectoryIndex::new(&index_base).freshness(Freshness::Ignored),
        source,
        &root,
    );

    let err = must_fail(
        installer.stage("greeter", &"^1.0".parse()?, &Lockfile::new()),
        "an install whose digest does not match",
    )?;
    assert!(
        format!("{err}").contains("not the expected"),
        "expected a digest mismatch, got: {err}"
    );
    assert!(
        !root.join("greeter").exists(),
        "a failed install must leave nothing in the plugin root"
    );
    Ok(())
}

#[test]
fn an_install_pins_and_the_registry_then_refuses_anything_else() -> TestResult {
    let temp = tempfile::tempdir()?;
    let built = temp.path().join("src/greeter");
    write_plugin(
        &built,
        "name = \"greeter\"\nversion = \"1.0.0\"\n\n[config]\ngreeting = \"hi\"\n",
        GREETER,
    )?;
    let (archive, digest) = pack(&built)?;

    let index_base = temp.path().join("index");
    write_index(
        &index_base,
        &one_release("greeter", "1.0.0", &digest, "fake://greeter:1.0.0")?,
    )?;

    let mut source = FakeSource::default();
    source.insert("fake://greeter:1.0.0", archive);

    let root = temp.path().join("plugins");
    let installer = Installer::new(
        DirectoryIndex::new(&index_base).freshness(Freshness::Ignored),
        source,
        &root,
    );

    let mut lockfile = Lockfile::new();
    let staged = installer.stage("greeter", &"^1.0".parse()?, &lockfile)?;
    assert!(
        staged.review().is_none(),
        "a first install has nothing to diff against"
    );
    assert!(
        staged.widens(),
        "a first install is a decision, not a continuation"
    );

    let pin = staged.commit()?;
    lockfile.pin("greeter", pin);
    assert!(root.join("greeter/init.lua").is_file());

    // The pinned plugin loads.
    let mut registry: Registry<GreeterClass> =
        Registry::isolated(Lua::new(), Sandbox::restricted()).with_lockfile(lockfile.clone());
    let report = registry.load_dir(&root)?;
    assert_eq!(
        report.loaded,
        vec!["greeter".to_string()],
        "{:?}",
        report.failures
    );

    // Editing a file after the fact breaks the pin, which is the whole point.
    fs::write(
        root.join("greeter/init.lua"),
        GREETER.replace("hello", "pwned"),
    )?;
    let mut registry: Registry<GreeterClass> =
        Registry::isolated(Lua::new(), Sandbox::restricted()).with_lockfile(lockfile);
    let report = registry.load_dir(&root)?;
    assert!(
        report.loaded.is_empty(),
        "a plugin off its pin must not load"
    );
    assert_eq!(report.failures.len(), 1);
    Ok(())
}

#[test]
fn an_unpinned_plugin_is_refused_once_a_lockfile_exists() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("plugins");
    write_plugin(
        &root.join("stowaway"),
        "name = \"stowaway\"\nversion = \"1.0.0\"\n",
        GREETER,
    )?;

    let mut registry: Registry<GreeterClass> =
        Registry::isolated(Lua::new(), Sandbox::restricted()).with_lockfile(Lockfile::new());
    let report = registry.load_dir(&root)?;
    assert!(report.loaded.is_empty());

    let failure = report.failures.first().ok_or("expected a failure")?;
    assert!(format!("{failure}").contains("not in the lockfile"));
    Ok(())
}

#[test]
fn an_upgrade_that_widens_authority_is_reported_before_it_is_installed() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("plugins");

    // Installed: no capabilities at all.
    write_plugin(
        &root.join("greeter"),
        "name = \"greeter\"\nversion = \"1.0.0\"\n",
        GREETER,
    )?;

    // Candidate: same author, same name, one more line in `[capabilities]`.
    let candidate = temp.path().join("src/greeter");
    write_plugin(
        &candidate,
        "name = \"greeter\"\nversion = \"1.1.0\"\n\n[capabilities.network]\nhosts = [\"evil.example\"]\n",
        GREETER,
    )?;
    let (archive, digest) = pack(&candidate)?;

    let index_base = temp.path().join("index");
    write_index(
        &index_base,
        &one_release("greeter", "1.1.0", &digest, "fake://greeter:1.1.0")?,
    )?;

    let mut source = FakeSource::default();
    source.insert("fake://greeter:1.1.0", archive);

    let installer = Installer::new(
        DirectoryIndex::new(&index_base).freshness(Freshness::Ignored),
        source,
        &root,
    );

    let staged = installer.stage_upgrade("greeter", &"^1.0".parse()?, &Lockfile::new())?;

    let review = staged
        .review()
        .ok_or("expected an installed version to diff against")?;
    assert!(review.widens(), "a new capability is a widening: {review}");
    assert!(
        review
            .concerns()
            .any(|change| format!("{change}").contains("+ capability `network`")),
        "the diff should name the capability: {review}"
    );

    // Refusing leaves the installed version exactly as it was.
    let staging = staged.staging_dir().to_path_buf();
    staged.discard()?;
    assert!(!staging.exists());
    let installed = fs::read_to_string(root.join("greeter/plugin.toml"))?;
    assert!(
        !installed.contains("capabilities"),
        "a refused upgrade must not touch the installed plugin"
    );
    Ok(())
}

#[test]
fn a_stale_index_is_refused_rather_than_believed() -> TestResult {
    let temp = tempfile::tempdir()?;
    let index_base = temp.path().join("index");
    write_index(
        &index_base,
        &PluginReleases {
            schema: 1,
            name: "greeter".to_string(),
            updated: Some("2020-01-01T00:00:00Z".to_string()),
            expires: Some("2020-01-08T00:00:00Z".to_string()),
            releases: Vec::new(),
        },
    )?;

    let index = DirectoryIndex::new(&index_base).freshness(Freshness::Required);
    let err = must_fail(index.releases("greeter"), "an expired index")?;
    assert!(matches!(err, IndexError::Stale { .. }), "{err}");

    // And an index with no expiry at all is refused under the strict setting, since a
    // document that never goes stale can be withheld forever.
    write_index(
        &index_base,
        &PluginReleases {
            schema: 1,
            name: "nolimit".to_string(),
            updated: None,
            expires: None,
            releases: Vec::new(),
        },
    )?;
    let err = must_fail(index.releases("nolimit"), "an index with no expiry")?;
    assert!(format!("{err}").contains("`expires`"), "{err}");
    Ok(())
}

#[test]
fn a_lockfile_round_trips_and_refuses_a_different_signer() -> TestResult {
    let temp = tempfile::tempdir()?;
    let dir = temp.path().join("greeter");
    write_plugin(&dir, "name = \"greeter\"\nversion = \"1.0.0\"\n", GREETER)?;
    let digest = DirectoryDigest::compute(&dir)?;
    let manifest = stanchion::registry::read_manifest(&dir)?;

    let mut lockfile = Lockfile::new();
    lockfile.pin(
        "greeter",
        stanchion::registry::LockedPlugin::from_digest(&digest).signed_by("repo:acme/plugins"),
    );

    let rendered = lockfile.to_toml()?;
    let reloaded = Lockfile::parse(&rendered)?;

    let trusted = stanchion::registry::Signer::verified("repo:acme/plugins", None);
    reloaded.check(&manifest, &digest, &trusted)?;

    let someone_else = stanchion::registry::Signer::verified("repo:attacker/plugins", None);
    let err = must_fail(
        reloaded.check(&manifest, &digest, &someone_else),
        "a different signer",
    )?;
    assert!(matches!(err, LockError::SignerMismatch { .. }), "{err}");

    let unsigned = stanchion::registry::Signer::Unsigned;
    assert!(
        reloaded.check(&manifest, &digest, &unsigned).is_err(),
        "an unsigned build must not satisfy a signer pin"
    );
    Ok(())
}

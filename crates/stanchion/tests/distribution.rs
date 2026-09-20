//! Distribution: packaging, an index, pinned installs, and upgrade review.
#![cfg(all(feature = "distribution", feature = "signatures"))]

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use stanchion::dist::index::{DirectoryIndex, Freshness, PluginReleases, Release};
use stanchion::dist::install::{PluginSource, SourceError};
use stanchion::dist::package::{self, Limits};
use stanchion::dist::{IndexError, Installer, PluginIndex};
use stanchion::registry::{DirectoryDigest, LockError, Lockfile, Registry, Sandbox};
use stanchion::lua_class;
use mlua::{Lua, Result as LuaResult};

#[lua_class]
pub trait Greeter {
    fn new(config: mlua::Table, deps: mlua::Table) -> LuaResult<Self>;
    fn greet(&self) -> LuaResult<String>;
}

/// A plugin directory, written from scratch so each test states its own fixture.
fn write_plugin(dir: &Path, manifest: &str, body: &str) {
    fs::create_dir_all(dir).expect("plugin dir");
    fs::write(dir.join("plugin.toml"), manifest).expect("manifest");
    fs::write(dir.join("init.lua"), body).expect("entry");
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
fn pack(dir: &Path) -> (Vec<u8>, DirectoryDigest) {
    let mut archive = Vec::new();
    let digest = package::pack(dir, &mut archive).expect("pack");
    (archive, digest)
}

/// Writes a one-plugin index under `base`.
fn write_index(base: &Path, releases: &PluginReleases) {
    let dir = base.join("v1/plugins");
    fs::create_dir_all(&dir).expect("index dir");
    let path = dir.join(format!("{}.json", releases.name));
    let raw = serde_json::to_string_pretty(releases).expect("serialise");
    fs::write(path, raw).expect("write index");
}

#[test]
fn packing_round_trips_the_directory_digest() {
    let temp = tempfile::tempdir().expect("temp");
    let source = temp.path().join("src/greeter");
    write_plugin(
        &source,
        "name = \"greeter\"\nversion = \"1.0.0\"\n",
        GREETER,
    );
    // A nested file, since the digest is over relative paths and folding order.
    fs::create_dir_all(source.join("lib")).expect("lib");
    fs::write(source.join("lib/util.lua"), "return {}").expect("util");

    let (archive, digest) = pack(&source);

    let out = temp.path().join("out");
    fs::create_dir_all(&out).expect("out");
    package::unpack(archive.as_slice(), &out, Limits::default()).expect("unpack");

    let round_tripped = DirectoryDigest::compute(&out).expect("digest");
    assert_eq!(
        digest.hex(),
        round_tripped.hex(),
        "the digest is over the directory, so it must survive a repack"
    );
    assert_eq!(round_tripped.len(), 3);
}

/// Builds a one-entry `tar.gz` with the path written straight into the header.
///
/// The `tar` crate refuses to *write* a `..` path, which is the right default and
/// also the reason this has to bypass it: an archive arriving from a registry was not
/// necessarily produced by a well-behaved packer, so the unpacker cannot assume one.
fn forged_archive(name: &str, entry_type: tar::EntryType, link: Option<&str>) -> Vec<u8> {
    let mut header = tar::Header::new_gnu();
    header.set_size(0);
    header.set_mode(0o644);
    header.set_entry_type(entry_type);
    {
        let gnu = header.as_gnu_mut().expect("a gnu header");
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
        builder
            .append(&header, std::io::empty())
            .expect("append a forged entry");
        builder.into_inner().expect("tar").finish().expect("gzip");
    }
    raw
}

#[test]
fn unpacking_refuses_path_traversal_and_symlinks() {
    let temp = tempfile::tempdir().expect("temp");

    let cases = [
        ("../escape.lua", tar::EntryType::Regular, None),
        ("/absolute.lua", tar::EntryType::Regular, None),
        ("nested/../../escape.lua", tar::EntryType::Regular, None),
        ("link.lua", tar::EntryType::Symlink, Some("/etc/passwd")),
        ("hard.lua", tar::EntryType::Link, Some("/etc/passwd")),
    ];

    for (index, (name, entry_type, link)) in cases.into_iter().enumerate() {
        let raw = forged_archive(name, entry_type, link);
        let out = temp.path().join(format!("out-{index}"));
        fs::create_dir_all(&out).expect("out");

        let result = package::unpack(raw.as_slice(), &out, Limits::default());
        assert!(
            result.is_err(),
            "`{name}` should be refused rather than written"
        );
        assert!(
            fs::read_dir(&out)
                .expect("read out")
                .next()
                .is_none(),
            "`{name}` left something behind in the destination"
        );
    }

    assert!(
        !temp.path().join("escape.lua").exists(),
        "nothing may be written outside the destination"
    );
}

#[test]
fn a_tampered_package_fails_the_digest_check() {
    let temp = tempfile::tempdir().expect("temp");
    let honest = temp.path().join("src/greeter");
    write_plugin(&honest, "name = \"greeter\"\nversion = \"1.0.0\"\n", GREETER);
    let (_, digest) = pack(&honest);

    // Same manifest, different code — the attack a signature over `plugin.toml`
    // alone would miss entirely.
    let swapped = temp.path().join("swapped/greeter");
    write_plugin(
        &swapped,
        "name = \"greeter\"\nversion = \"1.0.0\"\n",
        GREETER.replace("self.greeting", "\"pwned\"").as_str(),
    );
    let (tampered_archive, _) = pack(&swapped);

    let index_base = temp.path().join("index");
    write_index(
        &index_base,
        &PluginReleases {
            schema: 1,
            name: "greeter".to_string(),
            updated: None,
            expires: None,
            releases: vec![
                Release::new("1.0.0".parse().expect("version"), format!("sha256:{}", digest.hex()))
                    .from_source("fake://greeter:1.0.0"),
            ],
        },
    );

    let mut source = FakeSource::default();
    source.insert("fake://greeter:1.0.0", tampered_archive);

    let root = temp.path().join("plugins");
    let installer = Installer::new(
        DirectoryIndex::new(&index_base).freshness(Freshness::Ignored),
        source,
        &root,
    );

    let err = installer
        .stage("greeter", &"^1.0".parse().expect("req"), &Lockfile::new())
        .expect_err("the digest does not match, so the install must fail");
    assert!(
        format!("{err}").contains("not the expected"),
        "expected a digest mismatch, got: {err}"
    );
    assert!(
        !root.join("greeter").exists(),
        "a failed install must leave nothing in the plugin root"
    );
}

#[test]
fn an_install_pins_and_the_registry_then_refuses_anything_else() {
    let temp = tempfile::tempdir().expect("temp");
    let built = temp.path().join("src/greeter");
    write_plugin(
        &built,
        "name = \"greeter\"\nversion = \"1.0.0\"\n\n[config]\ngreeting = \"hi\"\n",
        GREETER,
    );
    let (archive, digest) = pack(&built);

    let index_base = temp.path().join("index");
    write_index(
        &index_base,
        &PluginReleases {
            schema: 1,
            name: "greeter".to_string(),
            updated: None,
            expires: None,
            releases: vec![
                Release::new("1.0.0".parse().expect("version"), format!("sha256:{}", digest.hex()))
                    .from_source("fake://greeter:1.0.0"),
            ],
        },
    );

    let mut source = FakeSource::default();
    source.insert("fake://greeter:1.0.0", archive);

    let root = temp.path().join("plugins");
    let installer = Installer::new(
        DirectoryIndex::new(&index_base).freshness(Freshness::Ignored),
        source,
        &root,
    );

    let mut lockfile = Lockfile::new();
    let staged = installer
        .stage("greeter", &"^1.0".parse().expect("req"), &lockfile)
        .expect("stage");
    assert!(staged.review().is_none(), "a first install has nothing to diff against");
    assert!(staged.widens(), "a first install is a decision, not a continuation");

    let pin = staged.commit().expect("commit");
    lockfile.pin("greeter", pin);
    assert!(root.join("greeter/init.lua").is_file());

    // The pinned plugin loads.
    let mut registry: Registry<GreeterClass> =
        Registry::isolated(Lua::new(), Sandbox::restricted()).with_lockfile(lockfile.clone());
    let report = registry.load_dir(&root).expect("load");
    assert_eq!(report.loaded, vec!["greeter".to_string()], "{:?}", report.failures);

    // Editing a file after the fact breaks the pin, which is the whole point.
    fs::write(root.join("greeter/init.lua"), GREETER.replace("hello", "pwned"))
        .expect("tamper");
    let mut registry: Registry<GreeterClass> =
        Registry::isolated(Lua::new(), Sandbox::restricted()).with_lockfile(lockfile);
    let report = registry.load_dir(&root).expect("load");
    assert!(report.loaded.is_empty(), "a plugin off its pin must not load");
    assert_eq!(report.failures.len(), 1);
}

#[test]
fn an_unpinned_plugin_is_refused_once_a_lockfile_exists() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path().join("plugins");
    write_plugin(
        &root.join("stowaway"),
        "name = \"stowaway\"\nversion = \"1.0.0\"\n",
        GREETER,
    );

    let mut registry: Registry<GreeterClass> =
        Registry::isolated(Lua::new(), Sandbox::restricted()).with_lockfile(Lockfile::new());
    let report = registry.load_dir(&root).expect("load");
    assert!(report.loaded.is_empty());
    assert!(
        format!("{}", report.failures.first().expect("a failure"))
            .contains("not in the lockfile")
    );
}

#[test]
fn an_upgrade_that_widens_authority_is_reported_before_it_is_installed() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path().join("plugins");

    // Installed: no capabilities at all.
    write_plugin(
        &root.join("greeter"),
        "name = \"greeter\"\nversion = \"1.0.0\"\n",
        GREETER,
    );

    // Candidate: same author, same name, one more line in `[capabilities]`.
    let candidate = temp.path().join("src/greeter");
    write_plugin(
        &candidate,
        "name = \"greeter\"\nversion = \"1.1.0\"\n\n[capabilities.network]\nhosts = [\"evil.example\"]\n",
        GREETER,
    );
    let (archive, digest) = pack(&candidate);

    let index_base = temp.path().join("index");
    write_index(
        &index_base,
        &PluginReleases {
            schema: 1,
            name: "greeter".to_string(),
            updated: None,
            expires: None,
            releases: vec![
                Release::new("1.1.0".parse().expect("version"), format!("sha256:{}", digest.hex()))
                    .from_source("fake://greeter:1.1.0"),
            ],
        },
    );

    let mut source = FakeSource::default();
    source.insert("fake://greeter:1.1.0", archive);

    let installer = Installer::new(
        DirectoryIndex::new(&index_base).freshness(Freshness::Ignored),
        source,
        &root,
    );

    let staged = installer
        .stage_upgrade("greeter", &"^1.0".parse().expect("req"), &Lockfile::new())
        .expect("stage");

    let review = staged.review().expect("an installed version to diff against");
    assert!(review.widens(), "a new capability is a widening: {review}");
    assert!(
        review
            .concerns()
            .any(|change| format!("{change}").contains("+ capability `network`")),
        "the diff should name the capability: {review}"
    );

    // Refusing leaves the installed version exactly as it was.
    let staging = staged.staging_dir().to_path_buf();
    staged.discard().expect("discard");
    assert!(!staging.exists());
    let installed = fs::read_to_string(root.join("greeter/plugin.toml")).expect("manifest");
    assert!(
        !installed.contains("capabilities"),
        "a refused upgrade must not touch the installed plugin"
    );
}

#[test]
fn a_stale_index_is_refused_rather_than_believed() {
    let temp = tempfile::tempdir().expect("temp");
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
    );

    let index = DirectoryIndex::new(&index_base).freshness(Freshness::Required);
    let err = index.releases("greeter").expect_err("expired");
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
    );
    let err = index.releases("nolimit").expect_err("no expiry");
    assert!(format!("{err}").contains("`expires`"), "{err}");
}

#[test]
fn a_lockfile_round_trips_and_refuses_a_different_signer() {
    let temp = tempfile::tempdir().expect("temp");
    let dir = temp.path().join("greeter");
    write_plugin(&dir, "name = \"greeter\"\nversion = \"1.0.0\"\n", GREETER);
    let digest = DirectoryDigest::compute(&dir).expect("digest");
    let manifest = stanchion::registry::Manifest::read(&dir).expect("manifest");

    let mut lockfile = Lockfile::new();
    lockfile.pin(
        "greeter",
        stanchion::registry::LockedPlugin::from_digest(&digest).signed_by("repo:acme/plugins"),
    );

    let rendered = lockfile.to_toml().expect("render");
    let reloaded = Lockfile::parse(&rendered).expect("parse");

    let trusted = stanchion::registry::Signer::verified("repo:acme/plugins", None);
    reloaded
        .check(&manifest, &digest, &trusted)
        .expect("the pinned signer must be accepted");

    let someone_else = stanchion::registry::Signer::verified("repo:attacker/plugins", None);
    let err = reloaded
        .check(&manifest, &digest, &someone_else)
        .expect_err("a different signer must be refused");
    assert!(matches!(err, LockError::SignerMismatch { .. }), "{err}");

    let unsigned = stanchion::registry::Signer::Unsigned;
    assert!(
        reloaded.check(&manifest, &digest, &unsigned).is_err(),
        "an unsigned build must not satisfy a signer pin"
    );
}

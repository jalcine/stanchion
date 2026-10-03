//! Fuzzes manifest parsing and the validation that gates it.
//!
//! A `plugin.toml` is attacker-controlled in exactly the same way an archive is: it
//! arrives inside a downloaded package and is parsed before anything about the plugin
//! has been decided. Parsing feeds `name` into [`validate_name`] and `entry` into the
//! path confinement check, both of which exist to stop a manifest naming
//! `../../etc/whatever` or a path outside its own directory.
//!
//! The assertion is that parsing and validation always return rather than panic, and
//! that a name or entry which validation *accepts* really is confined — the property
//! the rest of the loader depends on.
//!
//!     cargo +nightly fuzz run manifest

#![no_main]

use std::path::Path;

use libfuzzer_sys::fuzz_target;
use stanchion_abi::manifest::{Manifest, validate_name};

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };

    // Names are checked on their own: discovery validates a directory name before it
    // ever reads the manifest inside it.
    if validate_name(text).is_ok() {
        assert!(!text.is_empty(), "an empty name must not validate");
        assert!(
            !text.contains('/') && !text.contains('\\'),
            "a validated name must not contain a path separator: {text:?}"
        );
        assert!(
            text != "." && text != "..",
            "a validated name must not be a directory traversal: {text:?}"
        );
    }

    // Then the manifest as a whole. Parsing alone does not check the entry path;
    // `Manifest::validate` is what the loader calls and what confines it.
    let Ok(manifest) = toml::from_str::<Manifest>(text) else {
        return;
    };

    if manifest.validate().is_ok() {
        let entry = Path::new(&manifest.entry);
        assert!(
            !entry.is_absolute(),
            "a validated entry must not be absolute: {:?}",
            manifest.entry
        );
        assert!(
            !entry
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir)),
            "a validated entry must not climb out of its directory: {:?}",
            manifest.entry
        );
        assert!(
            validate_name(&manifest.name).is_ok(),
            "a validated manifest must have a validated name: {:?}",
            manifest.name
        );
    }
});

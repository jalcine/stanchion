//! Fuzzes `stanchion_dist::package::unpack` against arbitrary bytes.
//!
//! This is the first thing a downloaded plugin touches, so it is the first thing an
//! attacker controls end to end: the bytes arrive over the network and are handed
//! straight to a gzip decoder and a tar reader. `unpack` is what stands between those
//! bytes and the filesystem, and it is written to refuse traversal (`..`, absolute
//! paths, symlinks), duplicate entries and decompression bombs.
//!
//! What this target asserts is not "never errors" — almost every input is garbage and
//! an error is the correct answer. It asserts that `unpack` always *returns*: no
//! panic, no unwind, no abort, whatever the archive claims about itself. A panic here
//! is reachable by anyone who can serve a package.
//!
//! It also checks the one invariant that matters if unpacking does succeed: nothing
//! lands outside the destination directory.
//!
//!     cargo +nightly fuzz run unpack

#![no_main]

use std::path::Path;

use libfuzzer_sys::fuzz_target;
use stanchion_dist::package::{Limits, unpack};

/// Smaller than the production defaults so the fuzzer spends its time on parsing
/// paths and headers rather than on writing megabytes to a temp directory.
fn limits() -> Limits {
    Limits {
        entries: 64,
        total_bytes: 1 << 20,
        file_bytes: 1 << 18,
    }
}

/// Every path under `root`, relative to it, after a successful unpack.
fn escaped(root: &Path) -> Vec<std::path::PathBuf> {
    let mut outside = Vec::new();
    let Ok(canonical_root) = root.canonicalize() else {
        return outside;
    };
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            match path.canonicalize() {
                Ok(resolved) if !resolved.starts_with(&canonical_root) => {
                    outside.push(resolved);
                }
                _ => {}
            }
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                stack.push(path);
            }
        }
    }
    outside
}

fuzz_target!(|data: &[u8]| {
    let Ok(dir) = tempfile::tempdir() else {
        return;
    };

    // `unpack` requires an existing, empty directory.
    let into = dir.path().join("into");
    if std::fs::create_dir(&into).is_err() {
        return;
    }

    if unpack(data, &into, limits()).is_ok() {
        let outside = escaped(&into);
        assert!(
            outside.is_empty(),
            "unpack wrote outside its destination: {outside:?}"
        );
    }
});

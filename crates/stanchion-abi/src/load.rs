//! Bulk loading: how a registry hands a backend a verified group of plugins.
//!
//! Single loads go through [`PluginBackend`](crate::PluginBackend). When a
//! registry loads a whole root, it verifies provenance itself (signatures,
//! revocations, lockfile) and hands each backend the plugins of its type,
//! already read and bound to their digests. The backend wires them into
//! whatever sharing its isolation model needs and reports per-plugin
//! outcomes, so one bad plugin never stops the others.

use std::path::PathBuf;

use crate::callback::{HostSetup, Policy};
use crate::manifest::Manifest;
use crate::rocks::RocksConfig;
use crate::signature::{DirectoryDigest, Signer};

/// One verified plugin, ready for a backend to instantiate.
///
/// `entry_bytes` are the exact bytes the digest was checked against. The
/// backend builds from these rather than re-reading from disk, closing the
/// window between verification and loading.
pub struct LoadItem<'a> {
    /// The plugin's manifest.
    pub manifest: &'a Manifest,
    /// Who signed it (`Unsigned` when no signature was required).
    pub signer: Signer,
    /// The digest the directory verified against, when one was computed.
    pub digest: Option<DirectoryDigest>,
    /// The entry file's bytes, digest-bound as above.
    pub entry_bytes: Vec<u8>,
}

/// Everything a backend needs to instantiate a group, beyond the items.
pub struct LoadContext<'a> {
    /// Capabilities the host offers.
    pub setup: &'a HostSetup,
    /// Policy granting them per plugin.
    pub policy: &'a dyn Policy,
    /// Rock search paths to expose, when the host configured a tree.
    pub rock_paths: Vec<String>,
    /// The rock tree configuration, when one is configured. Backends
    /// enforcing a native ABI (e.g. Lua C modules) check it at load time.
    pub rocks: Option<&'a RocksConfig>,
    /// Constructor to call on each plugin's class table.
    pub constructor: &'a str,
}

/// One plugin's result from a group load.
pub enum GroupOutcome {
    /// The plugin instantiated; the registry stores the instance.
    Loaded {
        /// Plugin name.
        name: String,
        /// The live instance.
        instance: Box<dyn crate::backend::PluginInstance>,
        /// Capabilities actually granted, after policy ran.
        granted: Vec<String>,
    },
    /// The plugin did not load; the rest of the group still did.
    Failed {
        /// Plugin name.
        name: String,
        /// Directory it was discovered in.
        dir: PathBuf,
        /// Why, rendered for display.
        reason: String,
    },
}

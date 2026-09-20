//! Secure distribution of stanchion plugins: OCI artifacts, an index anyone can run,
//! and installs that are pinned before they are trusted.
//!
//! [Signatures](stanchion_registry::signature) answer "who produced these bytes?".
//! Distribution has to answer three more questions that a signature does not touch:
//!
//! | Question | Answered by |
//! | --- | --- |
//! | *Which* build is the legitimate one? | the [lockfile](stanchion_registry::lock) |
//! | Can the place it came from lie to me? | it can; the pin is checked after the fetch |
//! | Is this upgrade asking for more than the last one? | the [review](stanchion_registry::upgrade) |
//!
//! None of those are theoretical. Substituting a different build, downgrading to an
//! older signed-but-vulnerable one, and shipping a new version that quietly adds a
//! capability are all attacks that work perfectly well **with a valid signature**, and
//! two of them work with the author's real key and no compromise at all.
//!
//! # The shape of it
//!
//! ```text
//!   index (untrusted)          OCI registry (untrusted)
//!         │ name + requirement         │ tar.gz
//!         ▼                            ▼
//!   ┌───────────────────────────────────────────┐
//!   │ stage: unpack ‖ digest ‖ manifest ‖ review│   nothing installed yet
//!   └───────────────────────────────────────────┘
//!         │ a person, or a policy, accepts
//!         ▼
//!   lockfile pin  ──►  registry refuses anything else at load
//! ```
//!
//! Every box the bytes pass through is untrusted except the lockfile, which the host
//! writes and commits. That is the property worth having: a compromised registry, a
//! hostile mirror and a network attacker all end up at the same digest comparison.
//!
//! # Features
//!
//! The crate is dep-light by default so an *index publisher* can depend on it for the
//! document types alone — an index is two static JSON files, and generating them
//! should not require an HTTP or TLS stack.
//!
//! | Feature | Adds |
//! | --- | --- |
//! | *(none)* | index documents, [`PluginIndex`], [`DirectoryIndex`] |
//! | `package` | packing and unpacking `tar.gz` plugin packages, [`Installer`] |
//! | `oci` | `package` plus [`OciSource`], fetching from an OCI registry |
//! | `http` | reading an index over HTTPS |
//! | `client` | `oci` plus `http`: everything a host needs |
//!
//! # What this does not buy
//!
//! - **It does not make a plugin safe.** It makes the bytes you run the bytes you
//!   chose. Whether that choice was good is what [capabilities](stanchion_registry)
//!   and the sandbox are for.
//! - **A yank is not a revocation.** An index can stop offering a release; it cannot
//!   stop one already pinned from loading. That needs a
//!   [revocation list](stanchion_registry::Revocations), which is checked at load.
//! - **`[rocks]` are a second supply chain.** Nothing here signs, pins or unpacks a
//!   LuaRocks package, and a C rock is native code outside the sandbox entirely. An
//!   [upgrade review](stanchion_registry::upgrade) reports a newly declared rock as a
//!   widening for exactly that reason.

pub mod index;
pub mod package;

#[cfg(feature = "package")]
pub mod install;

#[cfg(feature = "oci")]
pub mod oci;

#[cfg(feature = "http")]
pub mod http;

pub use index::{
    Catalog, DirectoryIndex, Freshness, IndexDocument, IndexError, PluginIndex, PluginReleases,
    Release, CATALOG_PATH, INDEX_SCHEMA,
};
pub use package::{Limits, PackageError, ARTIFACT_TYPE, CONFIG_MEDIA_TYPE, LAYER_MEDIA_TYPE};

#[cfg(feature = "package")]
pub use install::{InstallError, Installer, PluginSource, SourceError, Staged};

#[cfg(feature = "oci")]
pub use oci::OciSource;

#[cfg(feature = "http")]
pub use http::HttpIndex;

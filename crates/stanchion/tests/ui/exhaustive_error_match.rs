//! `stanchion_abi::Error` is `#[non_exhaustive]`: a downstream crate must carry a
//! wildcard arm.
//!
//! Without the attribute, adding a variant is a silent breaking change that only
//! shows up as a build failure in whichever crate matched the enum — which is how
//! `stanchion-ffi-c` broke when `Manifest` landed.
#![allow(unused_imports)]

use stanchion_abi::Error;

fn code(err: &Error) -> i32 {
    match err {
        Error::UnknownPlugin(_) => 1,
        Error::Plugin { .. } => 2,
        Error::Runtime(_) => 3,
        Error::Io(_) => 4,
        Error::Config(_) => 5,
        Error::Capability { .. } => 6,
        Error::Reentrant => 7,
        Error::Wasm(_) => 8,
        Error::Manifest { .. } => 9,
    }
}

fn main() {}

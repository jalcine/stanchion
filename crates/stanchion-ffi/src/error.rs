//! The flat error type every binding surfaces, re-exported from [`stanchion_abi`].
//!
//! The conversions *from* `mlua::Error`, `LoadFailure` and `RegistryError` live
//! where their source types do (the `lua` feature and `stanchion-registry`
//! respectively), so the orphan rule is satisfied without this crate re-declaring
//! anything.

pub use stanchion_abi::{Error, Result};

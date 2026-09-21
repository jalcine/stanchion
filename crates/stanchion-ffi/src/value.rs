//! The one dynamic value type every binding shares, re-exported from
//! [`stanchion_abi`] along with its Lua conversion helpers.

pub use stanchion_abi::value::{table_to_map, value_to_toml};
pub use stanchion_abi::Value;

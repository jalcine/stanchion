#![allow(unused_imports)]

use stanchion_lua::lua_class;
use stanchion_lua::Result;

#[lua_class(bogus = "nope")]
pub trait UnknownClassOption {
    fn greet(&self) -> Result<String>;
}

fn main() {}

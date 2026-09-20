#![allow(unused_imports)]

use stanchion::lua_class;
use stanchion::mlua::Result;

#[lua_class(bogus = "nope")]
pub trait UnknownClassOption {
    fn greet(&self) -> Result<String>;
}

fn main() {}

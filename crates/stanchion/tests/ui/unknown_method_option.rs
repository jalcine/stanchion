#![allow(unused_imports)]

use stanchion::lua_class;
use stanchion::mlua::Result;

#[lua_class]
pub trait UnknownMethodOption {
    #[lua(bogus)]
    fn greet(&self) -> Result<String>;
}

fn main() {}

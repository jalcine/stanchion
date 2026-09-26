#![allow(unused_imports)]

use stanchion_lua::lua_class;
use stanchion_lua::Result;

#[lua_class]
pub trait UnknownMethodOption {
    #[lua(bogus)]
    fn greet(&self) -> Result<String>;
}

fn main() {}

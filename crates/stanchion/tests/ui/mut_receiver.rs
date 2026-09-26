#![allow(unused_imports)]

use stanchion_lua::lua_class;
use stanchion_lua::Result;

#[lua_class]
pub trait MutReceiver {
    fn greet(&mut self) -> Result<String>;
}

fn main() {}

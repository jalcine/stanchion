#![allow(unused_imports)]

use stanchion::lua_class;
use stanchion::mlua::Result;

#[lua_class]
pub trait MutReceiver {
    fn greet(&mut self) -> Result<String>;
}

fn main() {}

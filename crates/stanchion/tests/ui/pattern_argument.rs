#![allow(unused_imports)]

use stanchion::lua_class;
use stanchion::mlua::Result;

#[lua_class]
pub trait PatternArgument {
    fn greet(&self, (a, b): (u32, u32)) -> Result<String>;
}

fn main() {}

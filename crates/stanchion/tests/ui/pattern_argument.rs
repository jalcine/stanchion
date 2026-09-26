#![allow(unused_imports)]

use stanchion_lua::lua_class;
use stanchion_lua::Result;

#[lua_class]
pub trait PatternArgument {
    fn greet(&self, (a, b): (u32, u32)) -> Result<String>;
}

fn main() {}

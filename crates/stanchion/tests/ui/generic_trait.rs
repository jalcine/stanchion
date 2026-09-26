#![allow(unused_imports)]

use stanchion_lua::lua_class;
use stanchion_lua::Result;

#[lua_class]
pub trait Generic<T> {
    fn greet(&self, value: T) -> Result<String>;
}

fn main() {}

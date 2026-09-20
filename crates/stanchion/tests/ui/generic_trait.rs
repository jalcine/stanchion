#![allow(unused_imports)]

use stanchion::lua_class;
use stanchion::mlua::Result;

#[lua_class]
pub trait Generic<T> {
    fn greet(&self, value: T) -> Result<String>;
}

fn main() {}

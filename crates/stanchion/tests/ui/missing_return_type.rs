#![allow(unused_imports)]

use stanchion_lua::lua_class;

#[lua_class]
pub trait MissingReturn {
    fn greet(&self);
}

fn main() {}

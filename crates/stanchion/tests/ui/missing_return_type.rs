#![allow(unused_imports)]

use stanchion::lua_class;

#[lua_class]
pub trait MissingReturn {
    fn greet(&self);
}

fn main() {}

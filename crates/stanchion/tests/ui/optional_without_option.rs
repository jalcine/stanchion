#![allow(unused_imports)]

use stanchion::lua_class;
use stanchion::mlua::Result;

#[lua_class]
pub trait BadOptional {
    fn greet(&self) -> Result<String>;

    #[lua(optional)]
    fn hook(&self) -> Result<String>;
}

fn main() {}

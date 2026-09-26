#![allow(unused_imports)]

use stanchion_lua::lua_class;
use stanchion_lua::Result;

#[lua_class]
pub trait BadOptional {
    fn greet(&self) -> Result<String>;

    #[lua(optional)]
    fn hook(&self) -> Result<String>;
}

fn main() {}

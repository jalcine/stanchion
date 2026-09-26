#![allow(unused_imports)]

use stanchion_lua::lua_class;
use stanchion_lua::Result;

#[lua_class]
pub trait FieldArity {
    #[lua(field)]
    fn pair(&self, a: u32, b: u32) -> Result<()>;
}

fn main() {}

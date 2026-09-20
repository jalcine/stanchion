#![allow(unused_imports)]

use stanchion::lua_class;
use stanchion::mlua::Result;

#[lua_class]
pub trait FieldArity {
    #[lua(field)]
    fn pair(&self, a: u32, b: u32) -> Result<()>;
}

fn main() {}

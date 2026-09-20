#![allow(unused_imports)]

use stanchion::lua_class;
use stanchion::mlua::Result;

#[lua_class]
pub trait FieldWithoutSelf {
    #[lua(field)]
    fn version() -> Result<String>;
}

fn main() {}

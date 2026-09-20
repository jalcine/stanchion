#![allow(unused_imports)]

use stanchion::lua_class;
use stanchion::mlua::Result;

#[lua_class]
pub trait FieldAndFunction {
    #[lua(field, function)]
    fn greeting(&self) -> Result<String>;
}

fn main() {}

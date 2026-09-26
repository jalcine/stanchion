#![allow(unused_imports)]

use stanchion_lua::lua_class;
use stanchion_lua::Result;

#[lua_class]
pub trait FieldAndFunction {
    #[lua(field, function)]
    fn greeting(&self) -> Result<String>;
}

fn main() {}

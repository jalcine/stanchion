#![allow(unused_imports)]

use stanchion_lua::lua_class;
use stanchion_lua::Result;

#[lua_class]
pub trait FieldWithoutSelf {
    #[lua(field)]
    fn version() -> Result<String>;
}

fn main() {}

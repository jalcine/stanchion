#![allow(unused_imports)]

use stanchion::lua_class;
use stanchion::mlua::Result;

#[lua_class]
pub trait DefaultBody {
    fn greet(&self) -> Result<String> {
        Ok(String::new())
    }
}

fn main() {}

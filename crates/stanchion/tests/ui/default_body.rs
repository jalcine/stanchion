#![allow(unused_imports)]

use stanchion_lua::lua_class;
use stanchion_lua::Result;

#[lua_class]
pub trait DefaultBody {
    fn greet(&self) -> Result<String> {
        Ok(String::new())
    }
}

fn main() {}

//! The smallest useful thing: bind a Lua class to a Rust trait.
//!
//! No registry, no manifests — just the typed contract and what it catches.
//!
//! ```sh
//! cargo run -p stanchion --features lua54,vendored --example greeter
//! ```

use stanchion_lua::{Lua, Result};
use stanchion_lua::{load_class, lua_class};

/// The contract a Lua greeter must satisfy.
///
/// Receiverless functions land on `GreeterClass`; `&self` methods become the trait,
/// which stays dyn-compatible so a native Rust greeter can implement it too.
#[lua_class]
pub trait Greeter {
    /// `Greeter.new(greeting)`
    fn new(greeting: String) -> Result<Self>;

    /// `obj:greet(who)`
    fn greet(&self, who: String) -> Result<String>;

    /// `obj.greeting` — a field read, not a call.
    #[lua(field)]
    fn greeting(&self) -> Result<String>;

    /// Absent in the Lua below, so this yields `Ok(None)` rather than failing.
    #[lua(optional)]
    fn farewell(&self) -> Result<Option<String>>;
}

const GREETER: &str = r#"
local Greeter = {}
Greeter.__index = Greeter

function Greeter.new(greeting)
  return setmetatable({ greeting = greeting }, Greeter)
end

function Greeter:greet(who)
  return self.greeting .. ", " .. who
end

return Greeter
"#;

/// The same interface, implemented in Rust rather than Lua.
struct Shouty;

impl Greeter for Shouty {
    fn greet(&self, who: String) -> Result<String> {
        Ok(format!("HEY, {}", who.to_uppercase()))
    }
    fn greeting(&self) -> Result<String> {
        Ok("HEY".to_string())
    }
    fn farewell(&self) -> Result<Option<String>> {
        Ok(None)
    }
}

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let lua = Lua::new();

    let class: GreeterClass = load_class(&lua, GREETER, "greeter.lua")?;
    let greeter = class.new("hello".to_string())?;

    println!("greet   : {}", greeter.greet("world".to_string())?);
    println!("field   : {}", greeter.greeting()?);
    println!("optional: {:?}", greeter.farewell()?);

    // Lua-backed and native implementations are interchangeable.
    let everyone: Vec<Box<dyn Greeter>> = vec![Box::new(greeter), Box::new(Shouty)];
    for one in &everyone {
        println!("dyn     : {}", one.greet("world".to_string())?);
    }

    // The contract is checked when the class loads, not when a method is called.
    let broken = "return { describe = function() end }";
    match load_class::<GreeterClass>(&lua, broken, "broken.lua") {
        Ok(_) => println!("\nunexpected: a class without `new` was accepted"),
        Err(err) => println!("\nrejected at load: {err}"),
    }

    Ok(())
}

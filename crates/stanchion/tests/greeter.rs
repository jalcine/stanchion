//! The synchronous half of `#[lua_class]`: constructors, methods, fields, optionals.

use stanchion_lua::{Error, Lua, Result};
use stanchion_lua::{LuaObject, load_class, lua_class};

/// Tests report failures as errors rather than panicking, so a broken assumption
/// surfaces with its own message instead of a bare unwrap location.
type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

const SOURCE: &str = include_str!("lua/greeter.lua");

#[lua_class]
pub trait Greeter {
    /// `Greeter.new(greeting)` — no receiver, so it lands on `GreeterClass`.
    fn new(greeting: String) -> Result<Self>;

    /// `Greeter.describe()`
    fn describe() -> Result<String>;

    /// `obj:greet(who)`
    fn greet(&self, who: String) -> Result<String>;

    /// `obj.greeting`
    #[lua(field)]
    fn greeting(&self) -> Result<String>;

    /// `obj.greeting = value`
    #[lua(field)]
    fn set_greeting(&self, value: String) -> Result<()>;

    /// `obj.count`, renamed on the Rust side.
    #[lua(field, name = "count")]
    fn calls(&self) -> Result<u32>;

    /// Absent in `greeter.lua`, so this yields `Ok(None)`.
    #[lua(optional)]
    fn on_unload(&self) -> Result<Option<()>>;
}

fn class(lua: &Lua) -> Result<GreeterClass> {
    load_class(lua, SOURCE, "greeter.lua")
}

#[test]
fn calls_class_functions_and_methods() -> Result<()> {
    let lua = Lua::new();
    let class = class(&lua)?;
    assert_eq!(class.describe()?, "greets people");

    let greeter = class.new("hello".to_string())?;
    assert_eq!(greeter.greet("world".to_string())?, "hello, world");
    assert_eq!(greeter.greet("again".to_string())?, "hello, again");
    assert_eq!(greeter.calls()?, 2);
    Ok(())
}

#[test]
fn reads_and_writes_fields() -> Result<()> {
    let lua = Lua::new();
    let greeter = class(&lua)?.new("hello".to_string())?;
    assert_eq!(greeter.greeting()?, "hello");
    greeter.set_greeting("hi".to_string())?;
    assert_eq!(greeter.greeting()?, "hi");
    assert_eq!(greeter.greet("you".to_string())?, "hi, you");
    Ok(())
}

#[test]
fn missing_optional_method_is_none() -> Result<()> {
    let lua = Lua::new();
    let greeter = class(&lua)?.new("hello".to_string())?;
    assert_eq!(greeter.on_unload()?, None);
    Ok(())
}

#[test]
fn present_optional_method_is_called() -> Result<()> {
    let lua = Lua::new();
    let source = r#"
        local C = {}
        C.__index = C
        function C.new() return setmetatable({ unloaded = false }, C) end
        function C.describe() return "x" end
        function C:greet(who) return who end
        function C:on_unload() self.unloaded = true end
        return C
    "#;
    let class: GreeterClass = load_class(&lua, source, "with_hook.lua")?;
    let greeter = class.new(String::new())?;
    assert_eq!(greeter.on_unload()?, Some(()));
    Ok(())
}

#[test]
fn rejects_a_class_missing_a_constructor() -> TestResult {
    let lua = Lua::new();
    let Err(err) =
        load_class::<GreeterClass>(&lua, "return { describe = function() end }", "bad.lua")
    else {
        return Err("a class without `new` should be rejected".into());
    };
    assert!(
        matches!(&err, Error::FromLuaConversionError { message: Some(m), .. }
            if m.contains("missing required function `new`")),
        "unexpected error: {err}"
    );
    Ok(())
}

#[test]
fn rejects_an_instance_missing_a_method() -> TestResult {
    let lua = Lua::new();
    let source = r#"
        local C = {}
        function C.new() return {} end
        function C.describe() return "x" end
        return C
    "#;
    let class: GreeterClass = load_class(&lua, source, "bad_instance.lua")?;
    let Err(err) = class.new(String::new()) else {
        return Err("an instance without `greet` should be rejected".into());
    };
    assert!(
        err.to_string()
            .contains("missing required function `greet`"),
        "unexpected error: {err}"
    );
    Ok(())
}

/// A native implementation of the same interface, to prove the trait stays usable
/// independently of Lua.
struct Shouty;

impl Greeter for Shouty {
    fn greet(&self, who: String) -> Result<String> {
        Ok(format!("HEY, {}", who.to_uppercase()))
    }
    fn greeting(&self) -> Result<String> {
        Ok("HEY".to_string())
    }
    fn set_greeting(&self, _value: String) -> Result<()> {
        Ok(())
    }
    fn calls(&self) -> Result<u32> {
        Ok(0)
    }
    fn on_unload(&self) -> Result<Option<()>> {
        Ok(None)
    }
}

#[test]
fn lua_and_native_impls_mix_in_one_registry() -> Result<()> {
    let lua = Lua::new();
    let registry: Vec<Box<dyn Greeter>> = vec![
        Box::new(class(&lua)?.new("hello".to_string())?),
        Box::new(Shouty),
    ];

    let greetings: Vec<String> = registry
        .iter()
        .map(|g| g.greet("world".to_string()))
        .collect::<Result<_>>()?;
    assert_eq!(greetings, ["hello, world", "HEY, WORLD"]);
    Ok(())
}

/// A method compiled out by `#[cfg]` must also drop out of the required-key set,
/// otherwise a handle would be rejected for lacking a method Rust cannot call.
#[lua_class]
pub trait Partial {
    fn new() -> Result<Self>;
    fn always(&self) -> Result<String>;
    #[cfg(feature = "send")]
    fn only_with_send(&self) -> Result<String>;
}

#[test]
fn cfg_removes_a_method_from_the_required_set() -> TestResult {
    assert_eq!(
        <PartialHandle as LuaObject>::required_methods().contains(&"only_with_send"),
        cfg!(feature = "send"),
    );

    let lua = Lua::new();
    let source = r#"
        local C = {}
        C.__index = C
        function C.new() return setmetatable({}, C) end
        function C:always() return "here" end
        return C
    "#;
    let class: PartialClass = load_class(&lua, source, "partial.lua")?;

    match (class.new(), cfg!(feature = "send")) {
        (Err(err), true) => assert!(
            err.to_string()
                .contains("missing required function `only_with_send`"),
            "unexpected error: {err}"
        ),
        (Ok(instance), false) => assert_eq!(instance.always()?, "here"),
        (Ok(_), true) => {
            return Err("`only_with_send` should be required under the send feature".into());
        }
        (Err(err), false) => {
            return Err(format!("the plugin should load without the send feature: {err}").into());
        }
    }
    Ok(())
}

//! A `#[lua_class]` trait may return any error type that converts from `mlua::Error`.
//!
//! The delegation the macro emits converts on the way out, so a host with its own
//! error enum does not have to wrap every call site — and `mlua::Result` still works,
//! because the conversion is then the blanket `impl<T> From<T> for T`.

use std::fmt;

use stanchion_lua::mlua::Lua;
use stanchion_lua::{load_class, lua_class};

const SOURCE: &str = include_str!("lua/greeter.lua");

/// The kind of error a host would already have: its own variants, plus one for
/// whatever the interpreter reports.
#[derive(Debug)]
enum HostError {
    Lua(stanchion_lua::mlua::Error),
    #[expect(dead_code, reason = "present to prove the type is a real enum, not a wrapper")]
    Policy(String),
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lua(err) => write!(f, "lua: {err}"),
            Self::Policy(what) => write!(f, "policy: {what}"),
        }
    }
}

impl std::error::Error for HostError {}

impl From<stanchion_lua::mlua::Error> for HostError {
    fn from(err: stanchion_lua::mlua::Error) -> Self {
        Self::Lua(err)
    }
}

type HostResult<T> = Result<T, HostError>;

#[lua_class]
pub trait Greeter {
    fn new(greeting: String) -> HostResult<Self>;
    fn describe() -> HostResult<String>;
    fn greet(&self, who: String) -> HostResult<String>;

    #[lua(field)]
    fn greeting(&self) -> HostResult<String>;

    #[lua(field)]
    fn set_greeting(&self, value: String) -> HostResult<()>;

    #[lua(field, name = "count")]
    fn calls(&self) -> HostResult<u32>;

    #[lua(optional)]
    fn on_unload(&self) -> HostResult<Option<()>>;
}

#[test]
fn every_call_shape_returns_the_host_error_type() -> HostResult<()> {
    let lua = Lua::new();
    let class: GreeterClass = load_class(&lua, SOURCE, "greeter.lua")?;

    // Class function, constructor, method, field get, field set, optional method.
    assert_eq!(class.describe()?, "greets people");
    let greeter = class.new("hello".to_string())?;
    assert_eq!(greeter.greet("world".to_string())?, "hello, world");
    assert_eq!(greeter.greeting()?, "hello");
    greeter.set_greeting("hi".to_string())?;
    assert_eq!(greeter.greet("you".to_string())?, "hi, you");
    assert_eq!(greeter.calls()?, 2);
    assert_eq!(greeter.on_unload()?, None);
    Ok(())
}

#[test]
fn a_lua_failure_arrives_as_the_host_variant() -> Result<(), Box<dyn std::error::Error>> {
    let lua = Lua::new();
    let source = r#"
        local C = {}
        C.__index = C
        function C.new(greeting) return setmetatable({ greeting = greeting }, C) end
        function C.describe() return "x" end
        function C:greet(who) error("no greeting for " .. who) end
        return C
    "#;
    let class: GreeterClass = load_class(&lua, source, "angry.lua")?;
    let greeter = class.new("hello".to_string())?;

    let Err(err) = greeter.greet("world".to_string()) else {
        return Err("a raising Lua method should fail".into());
    };
    // The point of the conversion: the error is the host's type, matchable as such,
    // with the interpreter's message still inside it.
    let HostError::Lua(inner) = &err else {
        return Err(format!("expected the Lua variant: {err}").into());
    };
    assert!(
        inner.to_string().contains("no greeting for world"),
        "unexpected error: {inner}"
    );
    Ok(())
}

/// The async arms convert too, so the future resolves to the host's error type.
#[cfg(feature = "async")]
mod asynchronous {
    use super::{HostError, HostResult};
    use stanchion_lua::mlua::Lua;
    use stanchion_lua::{load_class, lua_class};

    #[lua_class]
    pub trait Fetcher {
        fn new(prefix: String) -> HostResult<Self>;
        async fn fetch(&self, url: String) -> HostResult<String>;

        #[lua(optional)]
        async fn warm_up(&self) -> HostResult<Option<String>>;
    }

    const SOURCE: &str = include_str!("lua/fetcher.lua");

    #[tokio::test]
    async fn an_awaited_call_returns_the_host_error_type() -> Result<(), Box<dyn std::error::Error>>
    {
        let lua = Lua::new();
        let fetch_body = lua.create_async_function(|_, url: String| async move {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            Ok(format!("<{url}>"))
        })?;
        lua.globals().set("fetch_body", fetch_body)?;

        let class: FetcherClass = load_class(&lua, SOURCE, "fetcher.lua")?;
        let fetcher = class.new("p:".to_string())?;
        let body: String = fetcher.fetch("u".to_string()).await?;
        assert!(body.contains("<u>"), "unexpected body: {body}");

        // And a raise inside the awaited call still lands in the Lua variant.
        let angry = r#"
            local C = {}
            C.__index = C
            function C.new(prefix) return setmetatable({ prefix = prefix }, C) end
            function C:fetch(url) error("refused " .. url) end
            return C
        "#;
        let class: FetcherClass = load_class(&lua, angry, "angry_fetcher.lua")?;
        let fetcher = class.new("p:".to_string())?;
        let Err(err) = fetcher.fetch("u".to_string()).await else {
            return Err("a raising Lua method should fail".into());
        };
        let HostError::Lua(inner) = &err else {
            return Err(format!("expected the Lua variant: {err}").into());
        };
        assert!(
            inner.to_string().contains("refused u"),
            "unexpected error: {inner}"
        );
        Ok(())
    }
}

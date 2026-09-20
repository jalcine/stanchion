//! Bind Lua-defined classes to Rust traits.
//!
//! This is the contract layer: the [`macro@lua_class`] attribute, the traits it
//! generates impls for, and the conversions they need. The plugin registry, LuaRocks
//! support and out-of-process hosting live in sibling crates, re-exported together by
//! the `stanchion` facade.
//!
//! A Lua "class" is a table used as a metatable with `__index`; instances are tables
//! whose metatable is that class. [`macro@lua_class`] takes a Rust trait describing
//! such a class and generates:
//!
//! * a **class handle** (`{Trait}Class`) exposing the class table's constructors,
//! * an **instance handle** (`{Trait}Handle`) implementing the trait by delegating
//!   to the Lua object,
//! * [`FromLua`] impls for both that validate the contract at the load boundary.
//!
//! The trait keeps only `&self` methods, so it stays dyn-compatible: a registry of
//! `Box<dyn Trait>` can mix Lua-backed and native Rust implementations.
//!
//! # Features
//!
//! * `async` — enables `mlua/async`; `async fn` in a `#[lua_class]` trait becomes a
//!   method returning [`BoxFuture`], driven by [`ObjectLike::call_async_method`].
//! * `send` — enables `mlua/send`; generated traits gain [`MaybeSend`] + [`MaybeSync`]
//!   supertraits (so `dyn Trait` is `Send + Sync`) and [`BoxFuture`] requires `Send`.
//!
//! [`ObjectLike::call_async_method`]: mlua::ObjectLike::call_async_method

use std::future::Future;
use std::pin::Pin;

pub use mlua;
pub use mlua::{MaybeSend, MaybeSync};
pub use stanchion_macros::lua_class;

use mlua::chunk::AsChunk;
use mlua::{FromLua, Lua, ObjectLike, Result, Table, Value};

/// A boxed future returned by `async` methods of a [`macro@lua_class`] trait.
///
/// Requires `Send` when the `send` feature is enabled, matching `mlua`'s own
/// [`MaybeSend`] convention.
#[cfg(feature = "send")]
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A boxed future returned by `async` methods of a [`macro@lua_class`] trait.
#[cfg(not(feature = "send"))]
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// A Lua object a handle can wrap: a table, or userdata.
///
/// `mlua`'s `ObjectLike` covers both but is sealed, so this enum re-dispatches the
/// operations generated code needs rather than implementing that trait.
#[derive(Clone, Debug)]
pub enum LuaHandle {
    /// A plain table, the usual shape for a Lua-defined class.
    Table(Table),
    /// Userdata, for a class whose state lives on the Rust side.
    UserData(mlua::AnyUserData),
}

impl LuaHandle {
    /// Reads a key, honouring `__index`.
    pub fn get<V: FromLua>(&self, key: impl mlua::IntoLua) -> Result<V> {
        match self {
            LuaHandle::Table(table) => table.get(key),
            LuaHandle::UserData(data) => data.get(key),
        }
    }

    /// Writes a key, honouring `__newindex`.
    pub fn set(&self, key: impl mlua::IntoLua, value: impl mlua::IntoLua) -> Result<()> {
        match self {
            LuaHandle::Table(table) => table.set(key, value),
            LuaHandle::UserData(data) => data.set(key, value),
        }
    }

    /// Calls `name` with the object as its first argument (Lua's `obj:name(..)`).
    pub fn call_method<R: mlua::FromLuaMulti>(
        &self,
        name: &str,
        args: impl mlua::IntoLuaMulti,
    ) -> Result<R> {
        match self {
            LuaHandle::Table(table) => table.call_method(name, args),
            LuaHandle::UserData(data) => data.call_method(name, args),
        }
    }

    /// Calls `name` without passing the object (Lua's `obj.name(..)`).
    pub fn call_function<R: mlua::FromLuaMulti>(
        &self,
        name: &str,
        args: impl mlua::IntoLuaMulti,
    ) -> Result<R> {
        match self {
            LuaHandle::Table(table) => table.call_function(name, args),
            LuaHandle::UserData(data) => data.call_function(name, args),
        }
    }

    /// Awaits `name` with the object as its first argument.
    #[cfg(feature = "async")]
    pub async fn call_async_method<R: mlua::FromLuaMulti>(
        &self,
        name: &str,
        args: impl mlua::IntoLuaMulti,
    ) -> Result<R> {
        match self {
            LuaHandle::Table(table) => table.call_async_method(name, args).await,
            LuaHandle::UserData(data) => data.call_async_method(name, args).await,
        }
    }

    /// Awaits `name` without passing the object.
    #[cfg(feature = "async")]
    pub async fn call_async_function<R: mlua::FromLuaMulti>(
        &self,
        name: &str,
        args: impl mlua::IntoLuaMulti,
    ) -> Result<R> {
        match self {
            LuaHandle::Table(table) => table.call_async_function(name, args).await,
            LuaHandle::UserData(data) => data.call_async_function(name, args).await,
        }
    }

    /// The table inside, when this handle wraps one.
    ///
    /// Features that manipulate metatables directly — the registry's `exports`
    /// proxies, for instance — need a real table and use this to say so.
    pub fn as_table(&self) -> Option<&Table> {
        match self {
            LuaHandle::Table(table) => Some(table),
            LuaHandle::UserData(_) => None,
        }
    }

    /// The userdata inside, when this handle wraps some.
    pub fn as_userdata(&self) -> Option<&mlua::AnyUserData> {
        match self {
            LuaHandle::Table(_) => None,
            LuaHandle::UserData(data) => Some(data),
        }
    }

    /// The handle as a plain Lua value.
    pub fn to_value(&self) -> Value {
        match self {
            LuaHandle::Table(table) => Value::Table(table.clone()),
            LuaHandle::UserData(data) => Value::UserData(data.clone()),
        }
    }

    /// Names the Lua type, for error messages.
    pub fn type_name(&self) -> &'static str {
        match self {
            LuaHandle::Table(_) => "table",
            LuaHandle::UserData(_) => "userdata",
        }
    }
}

impl From<Table> for LuaHandle {
    fn from(table: Table) -> Self {
        LuaHandle::Table(table)
    }
}

impl From<mlua::AnyUserData> for LuaHandle {
    fn from(data: mlua::AnyUserData) -> Self {
        LuaHandle::UserData(data)
    }
}

impl FromLua for LuaHandle {
    fn from_lua(value: Value, _lua: &Lua) -> Result<Self> {
        let from = value.type_name();
        match value {
            Value::Table(table) => Ok(LuaHandle::Table(table)),
            Value::UserData(data) => Ok(LuaHandle::UserData(data)),
            _ => Err(mlua::Error::FromLuaConversionError {
                from,
                to: "LuaHandle".to_string(),
                message: Some("expected a table or userdata".to_string()),
            }),
        }
    }
}

/// An instance of a Lua class, backed by a Lua table.
pub trait LuaObject: FromLua + Sized {
    /// Keys that must resolve to functions for an object to be a valid instance.
    ///
    /// Built at call time rather than stored as a const so that `#[cfg]` on a trait
    /// method removes its key along with the method.
    fn required_methods() -> Vec<&'static str>;

    /// Wraps a table or userdata, checking [`Self::required_methods`].
    fn from_handle(handle: LuaHandle) -> Result<Self>;

    /// The underlying Lua object.
    fn handle(&self) -> &LuaHandle;

    /// Unwraps into the underlying Lua object.
    fn into_handle(self) -> LuaHandle;

    /// Wraps a table, checking [`Self::required_methods`].
    fn from_table(table: Table) -> Result<Self> {
        Self::from_handle(LuaHandle::Table(table))
    }

    /// The underlying table, or `None` when this instance is userdata.
    fn table(&self) -> Option<&Table> {
        self.handle().as_table()
    }
}

/// A Lua class table, the thing constructors are called on.
pub trait LuaClass: FromLua + Sized {
    /// Name used in conversion errors.
    const CLASS_NAME: &'static str;

    /// Keys that must resolve to functions for a table to be a valid class.
    ///
    /// Built at call time rather than stored as a const so that `#[cfg]` on a trait
    /// method removes its key along with the method.
    fn required_functions() -> Vec<&'static str>;

    /// The instance type this class constructs.
    type Instance: LuaObject;

    /// Wraps a table or userdata, checking [`Self::required_functions`].
    fn from_handle(handle: LuaHandle) -> Result<Self>;

    /// The underlying Lua object.
    fn handle(&self) -> &LuaHandle;

    /// Wraps a table, checking [`Self::required_functions`].
    fn from_table(table: Table) -> Result<Self> {
        Self::from_handle(LuaHandle::Table(table))
    }

    /// The underlying table, or `None` when this class is userdata.
    fn table(&self) -> Option<&Table> {
        self.handle().as_table()
    }
}

/// Evaluates a chunk that returns a class table, validating it against `C`.
///
/// ```ignore
/// let class: GreeterClass = load_class(&lua, &source, "greeter.lua")?;
/// let greeter = class.new("hello".to_string())?;
/// ```
pub fn load_class<'a, C: LuaClass>(lua: &Lua, chunk: impl AsChunk + 'a, name: &str) -> Result<C> {
    lua.load(chunk).set_name(name).eval()
}

/// Implementation details used by generated code. Not a stable API.
#[doc(hidden)]
pub mod __private {
    pub use mlua;
    use mlua::{Error, Result, Table, Value};

    /// Unwraps a `Value` that is a table or userdata, or reports a typed error.
    pub fn expect_handle(value: Value, target: &'static str) -> Result<super::LuaHandle> {
        let from = value.type_name();
        match value {
            Value::Table(table) => Ok(super::LuaHandle::Table(table)),
            Value::UserData(data) => Ok(super::LuaHandle::UserData(data)),
            _ => Err(Error::FromLuaConversionError {
                from,
                to: target.to_string(),
                message: Some(format!("`{target}` must be backed by a table or userdata")),
            }),
        }
    }

    /// Checks that every name resolves to a function on a table or userdata.
    pub fn require_handle_functions(
        handle: &super::LuaHandle,
        target: &'static str,
        names: &[&str],
    ) -> Result<()> {
        for name in names {
            // Userdata with no methods has no `__index` at all, so probing it raises
            // rather than returning nil. Either way the method is not there.
            let message = match handle.get::<Value>(*name) {
                Ok(Value::Function(_)) => continue,
                Ok(Value::Nil) => format!("missing required function `{name}`"),
                Ok(other) => {
                    format!("`{name}` must be a function, found {}", other.type_name())
                }
                Err(err) => format!("missing required function `{name}` ({err})"),
            };
            return Err(Error::FromLuaConversionError {
                from: handle.type_name(),
                to: target.to_string(),
                message: Some(message),
            });
        }
        Ok(())
    }

    /// Unwraps a `Value` known to be a table, or reports a typed conversion error.
    pub fn expect_table(value: Value, target: &'static str) -> Result<Table> {
        let from = value.type_name();
        match value {
            Value::Table(table) => Ok(table),
            _ => Err(Error::FromLuaConversionError {
                from,
                to: target.to_string(),
                message: Some(format!("`{target}` must be backed by a Lua table")),
            }),
        }
    }

    /// Checks that every name resolves to a function, honouring `__index`.
    pub fn require_functions(table: &Table, target: &'static str, names: &[&str]) -> Result<()> {
        for name in names {
            let value: Value = table.get(*name)?;
            if !value.is_function() {
                return Err(Error::FromLuaConversionError {
                    from: "table",
                    to: target.to_string(),
                    message: Some(match value {
                        Value::Nil => format!("missing required function `{name}`"),
                        other => {
                            format!("`{name}` must be a function, found {}", other.type_name())
                        }
                    }),
                });
            }
        }
        Ok(())
    }

    /// Looks up an optional method, honouring `__index`.
    pub fn optional_function(
        handle: &super::LuaHandle,
        name: &str,
    ) -> Result<Option<mlua::Function>> {
        handle.get(name)
    }
}

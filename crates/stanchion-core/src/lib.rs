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
use mlua::{FromLua, Lua, Result, Table};

/// A boxed future returned by `async` methods of a [`macro@lua_class`] trait.
///
/// Requires `Send` when the `send` feature is enabled, matching `mlua`'s own
/// [`MaybeSend`] convention.
#[cfg(feature = "send")]
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A boxed future returned by `async` methods of a [`macro@lua_class`] trait.
#[cfg(not(feature = "send"))]
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// An instance of a Lua class, backed by a Lua table.
pub trait LuaObject: FromLua + Sized {
    /// Keys that must resolve to functions for a table to be a valid instance.
    ///
    /// Built at call time rather than stored as a const so that `#[cfg]` on a trait
    /// method removes its key along with the method.
    fn required_methods() -> Vec<&'static str>;

    /// Wraps a table, checking [`Self::required_methods`].
    fn from_table(table: Table) -> Result<Self>;

    /// The underlying Lua table.
    fn table(&self) -> &Table;

    /// Unwraps into the underlying Lua table.
    fn into_table(self) -> Table;
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

    /// Wraps a table, checking [`Self::required_functions`].
    fn from_table(table: Table) -> Result<Self>;

    /// The underlying Lua table.
    fn table(&self) -> &Table;
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
                        other => format!(
                            "`{name}` must be a function, found {}",
                            other.type_name()
                        ),
                    }),
                });
            }
        }
        Ok(())
    }

    /// Looks up an optional method, honouring `__index`.
    pub fn optional_function(table: &Table, name: &str) -> Result<Option<mlua::Function>> {
        table.get(name)
    }
}

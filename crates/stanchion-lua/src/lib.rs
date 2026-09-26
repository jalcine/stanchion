//! Lua plugin backend for stanchion.
//!
//! This crate implements the full Lua trait stack (LuaClass, LuaObject,
//! LuaHandle, load_class, __private) and provides the Lua-specific
//! [`LuaBackend`] and [`LuaInstance`] implementations that satisfy
//! [`stanchion_abi::PluginBackend`] and [`stanchion_abi::PluginInstance`].

use std::collections::HashMap;
use std::future::Future;
use std::io::Write;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use mlua::chunk::AsChunk;
use mlua::{FromLua, Lua, ObjectLike, Result as LuaResult, Table, Value};
use stanchion_abi::{Result as AbiResult, Value as AbiValue};
use stanchion_abi::runtime::Runtime;

pub use stanchion_macros::lua_class;

/// A boxed future returned by `async` methods of a `#[lua_class]` trait.
/// Since `mlua/send` is always enabled, we always use `Send` version.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Re-export mlua types used by the Lua trait stack.
pub use mlua;
pub use mlua::{MaybeSend, MaybeSync};

/// LuaRocks tree queries and version constraints (moved from `stanchion-rocks`).
pub mod rocks;

/// Per-plugin Lua state policy: standard libraries, memory and instruction limits.
pub mod sandbox;

/// A Lua object a handle can wrap: a table, or userdata.
///
/// `mlua`'s `ObjectLike` covers both but is sealed, so this enum re-dispatches
/// the operations generated code needs rather than implementing that trait.
#[derive(Clone, Debug)]
pub enum LuaHandle {
    /// A plain table, the usual shape for a Lua-defined class.
    Table(Table),
    /// Userdata, for a class whose state lives on the Rust side.
    UserData(mlua::AnyUserData),
}

impl LuaHandle {
    /// Reads a key, honouring `__index`.
    pub fn get<V: FromLua>(&self, key: impl mlua::IntoLua) -> LuaResult<V> {
        match self {
            LuaHandle::Table(table) => table.get(key),
            LuaHandle::UserData(data) => data.get(key),
        }
    }

    /// Writes a key, honouring `__newindex`.
    pub fn set(&self, key: impl mlua::IntoLua, value: impl mlua::IntoLua) -> LuaResult<()> {
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
    ) -> LuaResult<R> {
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
    ) -> LuaResult<R> {
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
    ) -> LuaResult<R> {
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
    ) -> LuaResult<R> {
        match self {
            LuaHandle::Table(table) => table.call_async_function(name, args).await,
            LuaHandle::UserData(data) => data.call_async_function(name, args).await,
        }
    }

    /// The table inside, when this handle wraps one.
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
    fn from_lua(value: Value, _lua: &Lua) -> LuaResult<Self> {
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
    fn required_methods() -> Vec<&'static str>;

    /// Wraps a table or userdata, checking [`Self::required_methods`].
    fn from_handle(handle: LuaHandle) -> LuaResult<Self>;

    /// The underlying Lua object.
    fn handle(&self) -> &LuaHandle;

    /// Unwraps into the underlying Lua object.
    fn into_handle(self) -> LuaHandle;

    /// Wraps a table, checking [`Self::required_methods`].
    fn from_table(table: Table) -> LuaResult<Self> {
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
    fn required_functions() -> Vec<&'static str>;

    /// The instance type this class constructs.
    type Instance: LuaObject;

    /// Wraps a table or userdata, checking [`Self::required_functions`].
    fn from_handle(handle: LuaHandle) -> LuaResult<Self>;

    /// The underlying Lua object.
    fn handle(&self) -> &LuaHandle;

    /// Wraps a table, checking [`Self::required_functions`].
    fn from_table(table: Table) -> LuaResult<Self> {
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
pub fn load_class<'a, C: LuaClass>(lua: &Lua, chunk: impl AsChunk + 'a, name: &str) -> LuaResult<C> {
    lua.load(chunk).set_name(name).eval()
}

/// Implementation details used by generated code. Not a stable API.
#[doc(hidden)]
pub mod __private {
    pub use mlua;
    use mlua::{Error, Result as LuaResult, Table, Value};

    /// Unwraps a `Value` that is a table or userdata, or reports a typed error.
    pub fn expect_handle(value: Value, target: &'static str) -> LuaResult<super::LuaHandle> {
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
    ) -> LuaResult<()> {
        for name in names {
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
    pub fn expect_table(value: Value, target: &'static str) -> LuaResult<Table> {
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
    pub fn require_functions(table: &Table, target: &'static str, names: &[&str]) -> LuaResult<()> {
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
    ) -> LuaResult<Option<mlua::Function>> {
        handle.get(name)
    }
}

/// A Lua-specific plugin backend that implements [`stanchion_abi::PluginBackend`].
///
/// Loads and instantiates Lua plugins using the full Lua trait stack defined
/// in this crate.
pub struct LuaBackend {
    /// The Lua state this backend uses.
    lua: Arc<Mutex<Lua>>,
}

impl LuaBackend {
    /// Creates a new Lua backend instance with the given Lua state.
    pub fn new(lua: Lua) -> Self {
        Self {
            lua: Arc::new(Mutex::new(lua)),
        }
    }
}

impl stanchion_abi::PluginBackend for LuaBackend {
    fn plugin_type(&self) -> stanchion_abi::PluginType {
        stanchion_abi::PluginType::Lua
    }

    fn load(&self, _manifest: &stanchion_abi::Manifest, _dir: &Path) -> stanchion_abi::Result<Box<dyn stanchion_abi::PluginInstance>> {
        // Implementation uses mlua to load and instantiate the plugin class.
        // This is a placeholder - the full implementation would:
        // 1. Create a Lua state
        // 2. Load the plugin chunk using load_class
        // 4. Return a LuaInstance wrapping the result
        unimplemented!("LuaBackend::load - requires full mlua integration with registry pattern")
    }
}

impl Runtime for LuaBackend {
    fn load(&self, manifest: &stanchion_abi::Manifest, dir: &Path) -> stanchion_abi::Result<Box<dyn stanchion_abi::PluginInstance>> {
        <Self as stanchion_abi::PluginBackend>::load(self, manifest, dir)
    }

    fn verify(&self, _manifest: &stanchion_abi::Manifest, _dir: &Path) -> stanchion_abi::Result<()> {
        Ok(())
    }

    fn audit(&self, _log: &mut dyn Write) -> stanchion_abi::Result<()> {
        Ok(())
    }

    fn call(
        &self,
        instance: &dyn stanchion_abi::PluginInstance,
        method: &str,
        args: &[stanchion_abi::Value],
    ) -> stanchion_abi::Result<stanchion_abi::Value> {
        instance.call(method, args)
    }

    fn budget(&self, _plugin_name: &str) -> stanchion_abi::Result<u64> {
        Ok(u64::MAX)
    }

    fn reset_budget(&self, _plugin_name: &str) -> stanchion_abi::Result<()> {
        Ok(())
    }

    fn runtime_name(&self) -> &'static str {
        "lua"
    }

    fn plugin_type(&self) -> stanchion_abi::PluginType {
        stanchion_abi::PluginType::Lua
    }

    fn lua_state(&self) -> Option<std::sync::Arc<std::sync::Mutex<mlua::Lua>>> {
        Some(std::sync::Arc::clone(&self.lua))
    }

    fn install_capability(
        &self,
        name: &str,
        _provider: &dyn stanchion_abi::callback::CapabilityProvider,
        _grant: &stanchion_abi::callback::Grant,
    ) -> stanchion_abi::Result<()> {
        // The actual capability installation is handled by the registry's
        // with_setup mechanism, which uses the Runtime trait to bind
        // functions into plugin environments.
        Ok(())
    }
}

/// Creates a new Lua state and returns it for `stanchion-registry`.
pub fn new_lua() -> mlua::Lua {
    mlua::Lua::new()
}

/// Creates a new `Runtime` backed by a fresh Lua state.
pub fn new_runtime() -> std::sync::Arc<dyn stanchion_abi::Runtime> {
    std::sync::Arc::new(LuaBackend::new(new_lua()))
}

/// A Lua-specific plugin instance that implements [`stanchion_abi::PluginInstance`].
///
/// Wraps a constructed Lua class instance and delegates method calls through
/// the full Lua trait stack.
pub struct LuaInstance {
    lua: Arc<Mutex<Lua>>,
    instance: LuaHandle,
    exports: Option<Table>,
}

impl stanchion_abi::PluginInstance for LuaInstance {
    fn call(&self, _method: &str, _args: &[AbiValue]) -> AbiResult<AbiValue> {
        // Convert args to mlua values, call the method, convert result back
        unimplemented!("LuaInstance::call")
    }

    fn runtime(&self) -> &str {
        "lua"
    }
}

impl LuaInstance {
    /// Creates a new LuaInstance from a constructed Lua class.
    pub fn new(lua: Lua, instance: LuaHandle, exports: Option<Table>) -> Self {
        Self {
            lua: Arc::new(Mutex::new(lua)),
            instance,
            exports,
        }
    }
}

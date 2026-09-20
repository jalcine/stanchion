//! A class contract with no required methods, for hosts that learn plugin shapes at
//! runtime rather than from a Rust trait.
//!
//! [`macro@crate::lua_class`] generates a contract from a trait's signatures, which a
//! generic process host cannot have: it is compiled before anyone writes a plugin. So
//! the out-of-process host loads plugins as [`DynClass`] — any table with a
//! constructor — and calls methods by name over RPC.
//!
//! The trade is that method names and argument types are checked when a call happens
//! rather than when the plugin loads. In-process hosts should prefer a real trait.

use mlua::{FromLua, Lua, MultiValue, Result, Value};

use stanchion_core::{LuaClass, LuaHandle, LuaObject};

/// A plugin class whose methods are resolved at call time.
#[derive(Clone, Debug)]
pub struct DynClass {
    handle: LuaHandle,
}

/// An instance whose methods are resolved at call time.
#[derive(Clone, Debug)]
pub struct DynInstance {
    handle: LuaHandle,
}

impl DynInstance {
    /// Calls a method by name, passing the instance as `self`.
    pub fn call_method(&self, method: &str, args: MultiValue) -> Result<Value> {
        self.handle.call_method(method, args)
    }

    /// Whether the instance resolves `method` to something callable.
    pub fn has_method(&self, method: &str) -> Result<bool> {
        Ok(self.handle.get::<Value>(method)?.is_function())
    }

    /// Calls a method asynchronously, passing the instance as `self`.
    #[cfg(feature = "async")]
    pub async fn call_method_async(&self, method: &str, args: MultiValue) -> Result<Value> {
        self.handle.call_async_method(method, args).await
    }
}

impl LuaObject for DynInstance {
    fn required_methods() -> Vec<&'static str> {
        Vec::new()
    }

    fn from_handle(handle: LuaHandle) -> Result<Self> {
        Ok(DynInstance { handle })
    }

    fn handle(&self) -> &LuaHandle {
        &self.handle
    }

    fn into_handle(self) -> LuaHandle {
        self.handle
    }
}

impl FromLua for DynInstance {
    fn from_lua(value: Value, _lua: &Lua) -> Result<Self> {
        let handle = stanchion_core::__private::expect_handle(value, "DynInstance")?;
        DynInstance::from_handle(handle)
    }
}

impl LuaClass for DynClass {
    const CLASS_NAME: &'static str = "dynamic";
    type Instance = DynInstance;

    fn required_functions() -> Vec<&'static str> {
        Vec::new()
    }

    fn from_handle(handle: LuaHandle) -> Result<Self> {
        Ok(DynClass { handle })
    }

    fn handle(&self) -> &LuaHandle {
        &self.handle
    }
}

impl FromLua for DynClass {
    fn from_lua(value: Value, _lua: &Lua) -> Result<Self> {
        let handle = stanchion_core::__private::expect_handle(value, "DynClass")?;
        DynClass::from_handle(handle)
    }
}

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

use mlua::{FromLua, Lua, MultiValue, ObjectLike, Result, Table, Value};

use stanchion_core::{LuaClass, LuaObject};

/// A plugin class whose methods are resolved at call time.
#[derive(Clone, Debug)]
pub struct DynClass {
    table: Table,
}

/// An instance whose methods are resolved at call time.
#[derive(Clone, Debug)]
pub struct DynInstance {
    table: Table,
}

impl DynInstance {
    /// Calls a method by name, passing the instance as `self`.
    pub fn call_method(&self, method: &str, args: MultiValue) -> Result<Value> {
        self.table.call_method(method, args)
    }

    /// Whether the instance resolves `method` to something callable.
    pub fn has_method(&self, method: &str) -> Result<bool> {
        Ok(self.table.get::<Value>(method)?.is_function())
    }

    /// Calls a method asynchronously, passing the instance as `self`.
    #[cfg(feature = "async")]
    pub async fn call_method_async(&self, method: &str, args: MultiValue) -> Result<Value> {
        self.table.call_async_method(method, args).await
    }
}

impl LuaObject for DynInstance {
    fn required_methods() -> Vec<&'static str> {
        Vec::new()
    }

    fn from_table(table: Table) -> Result<Self> {
        Ok(DynInstance { table })
    }

    fn table(&self) -> &Table {
        &self.table
    }

    fn into_table(self) -> Table {
        self.table
    }
}

impl FromLua for DynInstance {
    fn from_lua(value: Value, _lua: &Lua) -> Result<Self> {
        let table = stanchion_core::__private::expect_table(value, "DynInstance")?;
        DynInstance::from_table(table)
    }
}

impl LuaClass for DynClass {
    const CLASS_NAME: &'static str = "dynamic";
    type Instance = DynInstance;

    fn required_functions() -> Vec<&'static str> {
        Vec::new()
    }

    fn from_table(table: Table) -> Result<Self> {
        Ok(DynClass { table })
    }

    fn table(&self) -> &Table {
        &self.table
    }
}

impl FromLua for DynClass {
    fn from_lua(value: Value, _lua: &Lua) -> Result<Self> {
        let table = stanchion_core::__private::expect_table(value, "DynClass")?;
        DynClass::from_table(table)
    }
}

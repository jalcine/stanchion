//! The one dynamic value type every backend shares.
//!
//! A foreign caller cannot hand over a runtime-specific value — it has no host
//! state to make one in, and the lifetime would not survive the boundary. So
//! arguments and results travel as [`Value`], which each backend converts to and
//! from its own native representation.
//!
//! The shape deliberately matches what `plugin-host` already puts on the wire,
//! so an application can move between in-process and out-of-process hosting
//! without any plugin noticing. The one addition is that Lua's integer/float
//! split is preserved rather than collapsed the way JSON collapses it.

use std::cell::RefCell;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

thread_local! {
    static FUNCTION_CACHE: RefCell<Option<mlua::Value>> = RefCell::new(None);
}

/// A value crossing the boundary between a plugin and a foreign host.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    /// Lua `nil`.
    #[default]
    Nil,
    /// Lua `boolean`.
    Bool(bool),
    /// A Lua integer. Kept apart from [`Value::Float`] because Lua 5.3 onwards does.
    Int(i64),
    /// A Lua float.
    Float(f64),
    /// A Lua string. Must be UTF-8.
    Str(String),
    /// A table whose keys are exactly `1..=n`.
    List(Vec<Value>),
    /// Any other table, with keys rendered as strings.
    Map(BTreeMap<String, Value>),
    /// Lua function (in-process capability binding; cross-language sees as Nil).
    Function,
}

/// Converts a value into the TOML a grant narrows with.
///
/// Capability parameters are TOML on the manifest side, so a policy that narrows
/// a grant has to land back in TOML. TOML has no null, so a [`Value::Nil`] drops
/// the key rather than inventing one.
pub fn value_to_toml(value: &Value) -> Result<toml::Value, String> {
    Ok(match value {
        Value::Nil => return Err("TOML has no null, so a grant cannot contain one".to_string()),
        Value::Bool(value) => toml::Value::Boolean(*value),
        Value::Int(value) => toml::Value::Integer(*value),
        Value::Float(value) => toml::Value::Float(*value),
        Value::Str(value) => toml::Value::String(value.clone()),
        Value::List(items) => toml::Value::Array(
            items.iter().map(value_to_toml).collect::<Result<Vec<_>, _>>()?,
        ),
        Value::Map(entries) => {
            let mut table = toml::Table::new();
            for (key, entry) in entries {
                if matches!(entry, Value::Nil) {
                    continue;
                }
                table.insert(key.clone(), value_to_toml(entry)?);
            }
            toml::Value::Table(table)
        }
        Value::Function => return Err("TOML cannot represent a Lua function".to_string()),
    })
}

/// Converts manifest parameters into something a foreign policy can read.
pub fn toml_to_value(value: &toml::Value) -> Value {
    match value {
        toml::Value::String(value) => Value::Str(value.clone()),
        toml::Value::Integer(value) => Value::Int(*value),
        toml::Value::Float(value) => Value::Float(*value),
        toml::Value::Boolean(value) => Value::Bool(*value),
        toml::Value::Datetime(value) => Value::Str(value.to_string()),
        toml::Value::Array(items) => Value::List(items.iter().map(toml_to_value).collect()),
        toml::Value::Table(table) => Value::Map(
            table.iter().map(|(key, value)| (key.clone(), toml_to_value(value))).collect(),
        ),
    }
}

/// Converts a whole manifest table, the form a grant's parameters arrive in.
pub fn table_to_map(table: &toml::Table) -> Value {
    Value::Map(table.iter().map(|(key, value)| (key.clone(), toml_to_value(value))).collect())
}

#[cfg(feature = "lua")]
pub mod lua {
    use super::Value;
    use mlua::{Lua, LuaString, Table, Value as LuaValue};
    use std::collections::BTreeMap;
    use std::cell::RefCell;

    thread_local! {
        pub static FUNCTION_CACHE: RefCell<Option<LuaValue>> = RefCell::new(None);
    }

    /// Converts a `stanchion_abi::Value` to an `mlua::Value`.
    pub fn abi_to_lua(val: &Value, lua: &Lua) -> mlua::Result<LuaValue> {
        match val {
            Value::Nil => Ok(LuaValue::Nil),
            Value::Bool(v) => Ok(LuaValue::Boolean(*v)),
            Value::Int(v) => Ok(LuaValue::Integer(*v)),
            Value::Float(v) => Ok(LuaValue::Number(*v)),
            Value::Str(v) => Ok(LuaValue::String(lua.create_string(v)?)),
            Value::List(items) => {
                let table = lua.create_table()?;
                for (i, item) in items.iter().enumerate() {
                    table.set(i + 1, abi_to_lua(item, lua)?)?;
                }
                Ok(LuaValue::Table(table))
            }
            Value::Map(entries) => {
                let table = lua.create_table()?;
                for (k, v) in entries {
                    table.set(k.clone(), abi_to_lua(v, lua)?)?;
                }
                Ok(LuaValue::Table(table))
            }
            Value::Function => FUNCTION_CACHE.with(|cache| {
                cache.borrow_mut().take().map_or_else(
                    || Err(mlua::Error::RuntimeError("cached function not found".to_string())),
                    Ok,
                )
            }),
        }
    }

    /// Converts an `mlua::Value` to a `stanchion_abi::Value`.
    pub fn lua_to_abi(lua: &Lua, val: &LuaValue) -> Value {
        match val {
            LuaValue::Nil => Value::Nil,
            LuaValue::Boolean(v) => Value::Bool(*v),
            LuaValue::Integer(v) => Value::Int(*v),
            LuaValue::Number(v) => Value::Float(*v),
            LuaValue::String(s) => Value::Str(lua_string_to_string(s.clone())),
            LuaValue::Table(t) => Value::Map(lua_table_to_map(lua, t)),
            LuaValue::Function(f) => {
                FUNCTION_CACHE.with(|cache| {
                    *cache.borrow_mut() = Some(LuaValue::Function(f.clone()));
                });
                Value::Function
            }
            LuaValue::UserData(_) | LuaValue::Thread(_) | LuaValue::LightUserData(_) | LuaValue::Error(_) | LuaValue::Other(_) => {
                // FFI cannot represent these types, use Nil as fallback
                Value::Nil
            }
        }
    }

    fn lua_string_to_string(lua_string: LuaString) -> String {
        match lua_string.to_str() {
            Ok(s) => s.to_string(),
            Err(_) => String::new(),
        }
    }

    fn lua_table_to_map(lua: &Lua, lua_table: &Table) -> BTreeMap<String, Value> {
        let mut result = BTreeMap::new();
        for pair_result in lua_table.pairs::<mlua::Value, mlua::Value>() {
            if let Ok((k, v)) = pair_result {
                let key_str = match k {
                    LuaValue::String(s) => Some(lua_string_to_string(s)),
                    _ => None,
                };
                if let Some(key) = key_str {
                    result.insert(key, lua_to_abi(lua, &v));
                }
            }
        }
        result
    }
}
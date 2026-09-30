//! Conversions between [`stanchion_abi::Value`] and `mlua` values.
//!
//! These live here — not in `stanchion-abi` — because they name `mlua`
//! types, which only this crate may link. Hosts pass [`Value`](stanchion_abi::Value)s
//! to [`Runtime`](stanchion_abi::Runtime) methods and never convert directly.

use std::cell::RefCell;
use std::collections::BTreeMap;

use mlua::{Lua, LuaString, Table, Value as LuaValue};
use stanchion_abi::Value;

thread_local! {
    /// Carries a capability function across the provider boundary.
    ///
    /// Providers answer with [`Value::Function`], which carries no data by
    /// itself; the real `mlua::Function` waits here for the backend to bind
    /// it into the plugin's environment. Scoped to one bind: [`take_function`]
    /// removes it, so a stale function can never leak into another plugin.
    static FUNCTION_CACHE: RefCell<Option<LuaValue>> = const { RefCell::new(None) };
}

/// Stashes a function for [`take_function`], reporting it as [`Value::Function`].
pub fn stash_function(function: LuaValue) -> Value {
    FUNCTION_CACHE.with(|cache| {
        *cache.borrow_mut() = Some(function);
    });
    Value::Function
}

/// Takes the function [`stash_function`] left, or errors when none is waiting.
pub fn take_function() -> mlua::Result<LuaValue> {
    FUNCTION_CACHE.with(|cache| {
        cache.borrow_mut().take().ok_or_else(|| {
            mlua::Error::RuntimeError("cached function not found".to_string())
        })
    })
}

/// Converts a [`stanchion_abi::Value`] into the state `lua`.
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
                table.set(i.saturating_add(1), abi_to_lua(item, lua)?)?;
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
        Value::Function => take_function(),
    }
}

/// Converts an `mlua` value into a [`stanchion_abi::Value`].
///
/// Types with no ABI representation (userdata, threads, errors) become
/// [`Value::Nil`]; only string-keyed table entries cross, mirroring the
/// [`Value::Map`] contract.
pub fn lua_to_abi(lua: &Lua, val: &LuaValue) -> Value {
    match val {
        LuaValue::Nil => Value::Nil,
        LuaValue::Boolean(v) => Value::Bool(*v),
        LuaValue::Integer(v) => Value::Int(*v),
        LuaValue::Number(v) => Value::Float(*v),
        LuaValue::String(s) => Value::Str(lua_string_to_string(s.clone())),
            LuaValue::Table(t) => lua_table_to_value(lua, t),
        LuaValue::Function(f) => {
            stash_function(LuaValue::Function(f.clone()));
            Value::Function
        }
        LuaValue::UserData(_)
        | LuaValue::Thread(_)
        | LuaValue::LightUserData(_)
        | LuaValue::Error(_)
        | LuaValue::Other(_) => Value::Nil,
    }
}

fn lua_string_to_string(lua_string: LuaString) -> String {
    match lua_string.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => String::new(),
    }
}

/// Converts a table, preserving array shape: integer keys exactly `1..=n`
/// become a [`Value::List`]; anything else becomes a [`Value::Map`] of the
/// string-keyed entries, with other keys dropped as documented there.
fn lua_table_to_value(lua: &Lua, lua_table: &Table) -> Value {
    let mut seq: Vec<(i64, Value)> = Vec::new();
    let mut map = BTreeMap::new();
    let mut only_integers = true;
    for (k, v) in lua_table.pairs::<mlua::Value, mlua::Value>().flatten() {
        let converted = lua_to_abi(lua, &v);
        match k {
            LuaValue::Integer(index) if index >= 1 => seq.push((index, converted)),
            LuaValue::String(s) => {
                only_integers = false;
                map.insert(lua_string_to_string(s), converted);
            }
            _ => {
                only_integers = false;
            }
        }
    }
    if only_integers && !seq.is_empty() {
        seq.sort_by_key(|(index, _)| *index);
        let mut values = Vec::with_capacity(seq.len());
        let mut dense = true;
        for (position, (index, value)) in seq.into_iter().enumerate() {
            if dense && index == position as i64 + 1 {
                values.push(value);
            } else {
                // A sparse integer-keyed table is not a list; render the
                // keys as strings per the `Value::Map` contract instead of
                // dropping entries on the floor.
                dense = false;
                map.insert(index.to_string(), value);
            }
        }
        // Entries already pushed stay in order only when every key was
        // dense; otherwise the map above carries everything.
        if dense {
            return Value::List(values);
        }
        for (position, value) in values.into_iter().enumerate() {
            map.insert((position as i64 + 1).to_string(), value);
        }
    }
    Value::Map(map)
}

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
///
/// An **empty** table becomes an empty [`Value::List`]. Lua gives a table no marker
/// saying whether it is a sequence, so an empty one is genuinely both and something
/// has to be picked. A list is picked because it is what a host that sent an empty
/// list gets back — previously this fell through to [`Value::Map`], making an empty
/// list the one value whose *type* changed by crossing the boundary. The cost is that
/// an empty map arrives as an empty list; both cases are asserted in the tests below.
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
    if only_integers && seq.is_empty() {
        // Nothing to disambiguate: see the note on this function.
        return Value::List(Vec::new());
    }
    if only_integers && !seq.is_empty() {
        seq.sort_by_key(|(index, _)| *index);
        let mut values = Vec::with_capacity(seq.len());
        let mut dense = true;
        for (position, (index, value)) in seq.into_iter().enumerate() {
            // Lua lists are 1-based. `position` is an `enumerate` index, so the
            // conversion and the increment cannot realistically overflow, but the
            // workspace denies unchecked arithmetic rather than relying on that:
            // a `usize` that does not fit an `i64` simply never matches a Lua key.
            let expected = i64::try_from(position)
                .ok()
                .and_then(|p| p.checked_add(1));
            if dense && Some(index) == expected {
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
            map.insert(position.saturating_add(1).to_string(), value);
        }
    }
    Value::Map(map)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

    /// Round-trips a value through a real Lua state.
    fn round_trip(value: &Value) -> Fallible<Value> {
        let lua = Lua::new();
        let bound = abi_to_lua(value, &lua)?;
        Ok(lua_to_abi(&lua, &bound))
    }

    /// An empty [`Value::List`] came back as an empty [`Value::Map`].
    ///
    /// A Lua table carries no "this is a sequence" marker, so an empty one is
    /// genuinely ambiguous — but something has to be chosen, and `lua_table_to_value`
    /// fell through to `Map` because its list branch is guarded on a non-empty
    /// sequence. A host that sends a plugin an empty list gets a map back, which is
    /// the one case where a round-trip changes a value's *type*.
    #[test]
    fn an_empty_list_round_trips_as_a_list() -> Fallible<()> {
        assert_eq!(round_trip(&Value::List(Vec::new()))?, Value::List(Vec::new()));
        Ok(())
    }

    #[test]
    fn a_populated_list_round_trips() -> Fallible<()> {
        let value = Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
        assert_eq!(round_trip(&value)?, value);
        Ok(())
    }

    #[test]
    fn a_populated_map_round_trips() -> Fallible<()> {
        let mut map = BTreeMap::new();
        map.insert("k".to_string(), Value::Bool(false));
        let value = Value::Map(map);
        assert_eq!(round_trip(&value)?, value);
        Ok(())
    }

    /// The flip side of the choice above: an empty map also arrives as an empty list,
    /// because by then the two are the same Lua table. Asserted so the trade is
    /// recorded rather than discovered.
    #[test]
    fn an_empty_map_round_trips_as_a_list() -> Fallible<()> {
        assert_eq!(
            round_trip(&Value::Map(BTreeMap::new()))?,
            Value::List(Vec::new())
        );
        Ok(())
    }

    /// A sparse integer-keyed table is not a list; keys are kept as strings.
    #[test]
    fn a_sparse_integer_table_becomes_a_map() -> Fallible<()> {
        let lua = Lua::new();
        let table = lua.create_table()?;
        table.set(1, "a")?;
        table.set(3, "c")?;
        let value = lua_to_abi(&lua, &LuaValue::Table(table));
        match value {
            Value::Map(map) => {
                assert_eq!(map.get("1"), Some(&Value::Str("a".to_string())));
                assert_eq!(map.get("3"), Some(&Value::Str("c".to_string())));
            }
            other => return Err(format!("expected a map, got {other:?}").into()),
        }
        Ok(())
    }
}

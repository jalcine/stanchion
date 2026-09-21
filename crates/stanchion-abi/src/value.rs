//! The one dynamic value type every backend shares.
//!
//! A foreign caller cannot hand over an `mlua::Value` — it has no Lua state to
//! make one in, and the lifetime would not survive the boundary. So arguments
//! and results travel as [`Value`], which each backend converts to and from its
//! own natives.
//!
//! The shape deliberately matches what `plugin-host` already puts on the wire,
//! so an application can move between in-process and out-of-process hosting
//! without any plugin noticing. The one addition is that Lua's integer/float
//! split is preserved rather than collapsed the way JSON collapses it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

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
    /// A Lua string. Must be UTF-8; see [`Value::from_lua`].
    Str(String),
    /// A table whose keys are exactly `1..=n`.
    List(Vec<Value>),
    /// Any other table, with keys rendered as strings.
    Map(BTreeMap<String, Value>),
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

/// The Lua-specific half of [`Value`], kept behind the `lua` feature.
#[cfg(feature = "lua")]
pub mod lua {
    use std::collections::BTreeMap;

    use mlua::{Lua, Value as LuaValue};

    use super::Value;

    /// How deep a Lua table may nest before conversion gives up.
    ///
    /// Lua tables can contain themselves. Nothing stops a plugin returning `t`
    /// where `t.self = t`, and a naive recursive walk of that overflows the
    /// stack and takes the host process with it — a plugin crashing its host is
    /// exactly what the rest of this project exists to prevent, so the walk is
    /// bounded.
    const MAX_DEPTH: usize = 64;

    impl Value {
        /// Builds the Lua value this describes.
        pub fn to_lua(&self, lua: &Lua) -> mlua::Result<LuaValue> {
            self.to_lua_at(lua, 0)
        }

        fn to_lua_at(&self, lua: &Lua, depth: usize) -> mlua::Result<LuaValue> {
            if depth > MAX_DEPTH {
                return Err(mlua::Error::RuntimeError(format!(
                    "value nests deeper than {MAX_DEPTH} levels"
                )));
            }
            Ok(match self {
                Value::Nil => LuaValue::Nil,
                Value::Bool(value) => LuaValue::Boolean(*value),
                Value::Int(value) => LuaValue::Integer(*value),
                Value::Float(value) => LuaValue::Number(*value),
                Value::Str(value) => LuaValue::String(lua.create_string(value)?),
                Value::List(items) => {
                    let table = lua.create_table_with_capacity(items.len(), 0)?;
                    for (index, item) in items.iter().enumerate() {
                        // Lua indexes from one, and `enumerate` from zero.
                        let position = index.saturating_add(1);
                        table.raw_set(position, item.to_lua_at(lua, depth.saturating_add(1))?)?;
                    }
                    LuaValue::Table(table)
                }
                Value::Map(entries) => {
                    let table = lua.create_table_with_capacity(0, entries.len())?;
                    for (key, entry) in entries {
                        table.raw_set(
                            lua.create_string(key)?,
                            entry.to_lua_at(lua, depth.saturating_add(1))?,
                        )?;
                    }
                    LuaValue::Table(table)
                }
            })
        }

        /// Reads a Lua value.
        ///
        /// Three kinds of value have no representation here and are refused
        /// rather than silently flattened: functions, threads and userdata. A
        /// plugin returning one has written for an in-process Rust host, and
        /// saying so is more useful than handing the caller a `nil` it will
        /// misread.
        ///
        /// Lua strings are byte strings, so one that is not UTF-8 is also
        /// refused. The alternative is lossy replacement, which corrupts the
        /// value on its way through a boundary whose whole job is to carry it
        /// faithfully.
        pub fn from_lua(value: &LuaValue) -> mlua::Result<Value> {
            Value::from_lua_at(value, 0)
        }

        fn from_lua_at(value: &LuaValue, depth: usize) -> mlua::Result<Value> {
            if depth > MAX_DEPTH {
                return Err(mlua::Error::RuntimeError(format!(
                    "table nests deeper than {MAX_DEPTH} levels, or contains itself"
                )));
            }
            Ok(match value {
                LuaValue::Nil => Value::Nil,
                LuaValue::Boolean(value) => Value::Bool(*value),
                LuaValue::Integer(value) => Value::Int(*value),
                LuaValue::Number(value) => Value::Float(*value),
                LuaValue::String(value) => Value::Str(value.to_str()?.to_string()),
                LuaValue::Table(table) => table_to_value(table, depth)?,
                other => {
                    return Err(mlua::Error::RuntimeError(format!(
                        "a Lua {} cannot cross a language boundary",
                        other.type_name()
                    )));
                }
            })
        }
    }

    /// Decides whether a table is a sequence or a mapping, and converts it.
    ///
    /// Lua has one table type doing both jobs, so the distinction has to be
    /// inferred: keys of exactly `1..=n` make a list, anything else a map. Two
    /// consequences worth knowing, both documented in `docs/bindings.md`:
    ///
    /// * An empty table becomes an empty [`Value::List`]. Lua cannot tell the
    ///   two apart, so neither can this.
    /// * A sparse array — `{[1] = "a", [3] = "c"}` — becomes a map with the keys
    ///   `"1"` and `"3"`, because it is not a sequence and pretending otherwise
    ///   would drop the third element.
    fn table_to_value(table: &mlua::Table, depth: usize) -> mlua::Result<Value> {
        let next = depth.saturating_add(1);

        let length = table.raw_len();
        // Starts true so an empty table settles as an empty list:
        // `pairs.len() == length` holds at zero. Any key outside `1..=length`
        // clears it below.
        let mut is_sequence = true;
        let mut pairs = Vec::new();

        for pair in table.pairs::<LuaValue, LuaValue>() {
            let (key, value) = pair?;
            if is_sequence {
                let within = match &key {
                    LuaValue::Integer(index) => *index >= 1 && (*index as usize) <= length,
                    _ => false,
                };
                if !within {
                    is_sequence = false;
                }
            }
            pairs.push((key, Value::from_lua_at(&value, next)?));
        }

        if is_sequence && pairs.len() == length {
            let mut items = vec![Value::Nil; length];
            for (key, value) in pairs {
                if let LuaValue::Integer(index) = key {
                    let position = (index as usize).saturating_sub(1);
                    if let Some(slot) = items.get_mut(position) {
                        *slot = value;
                    }
                }
            }
            return Ok(Value::List(items));
        }

        let mut entries = BTreeMap::new();
        for (key, value) in pairs {
            entries.insert(key_to_string(&key)?, value);
        }
        Ok(Value::Map(entries))
    }

    /// Renders a table key as a string, since no target language has Lua's key freedom.
    fn key_to_string(key: &LuaValue) -> mlua::Result<String> {
        Ok(match key {
            LuaValue::String(key) => key.to_str()?.to_string(),
            LuaValue::Integer(key) => key.to_string(),
            LuaValue::Number(key) => key.to_string(),
            LuaValue::Boolean(key) => key.to_string(),
            other => {
                return Err(mlua::Error::RuntimeError(format!(
                    "a table keyed by {} cannot cross a language boundary",
                    other.type_name()
                )));
            }
        })
    }
}

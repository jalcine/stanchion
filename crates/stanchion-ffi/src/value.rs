//! The one dynamic value type every binding shares.
//!
//! A foreign caller cannot hand over an `mlua::Value` — it has no Lua state to make
//! one in, and the lifetime would not survive the boundary. So arguments and results
//! travel as [`Value`], which each binding converts to and from its own natives.
//!
//! The shape deliberately matches what `plugin-host` already puts on the wire, so an
//! application can move between in-process and out-of-process hosting without any
//! plugin noticing. The one addition is that Lua's integer/float split is preserved
//! rather than collapsed the way JSON collapses it.

use std::collections::BTreeMap;

use mlua::{Lua, Value as Lua_};
use serde::{Deserialize, Serialize};

/// How deep a Lua table may nest before conversion gives up.
///
/// Lua tables can contain themselves. Nothing stops a plugin returning `t` where
/// `t.self = t`, and a naive recursive walk of that overflows the stack and takes the
/// host process with it — a plugin crashing its host is exactly what the rest of this
/// project exists to prevent, so the walk is bounded.
const MAX_DEPTH: usize = 64;

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

impl Value {
    /// Builds the Lua value this describes.
    pub fn to_lua(&self, lua: &Lua) -> mlua::Result<Lua_> {
        self.to_lua_at(lua, 0)
    }

    fn to_lua_at(&self, lua: &Lua, depth: usize) -> mlua::Result<Lua_> {
        if depth > MAX_DEPTH {
            return Err(mlua::Error::RuntimeError(format!(
                "value nests deeper than {MAX_DEPTH} levels"
            )));
        }
        Ok(match self {
            Value::Nil => Lua_::Nil,
            Value::Bool(value) => Lua_::Boolean(*value),
            Value::Int(value) => Lua_::Integer(*value),
            Value::Float(value) => Lua_::Number(*value),
            Value::Str(value) => Lua_::String(lua.create_string(value)?),
            Value::List(items) => {
                let table = lua.create_table_with_capacity(items.len(), 0)?;
                for (index, item) in items.iter().enumerate() {
                    // Lua indexes from one, and `enumerate` from zero.
                    let position = index.saturating_add(1);
                    table.raw_set(position, item.to_lua_at(lua, depth.saturating_add(1))?)?;
                }
                Lua_::Table(table)
            }
            Value::Map(entries) => {
                let table = lua.create_table_with_capacity(0, entries.len())?;
                for (key, entry) in entries {
                    table.raw_set(
                        lua.create_string(key)?,
                        entry.to_lua_at(lua, depth.saturating_add(1))?,
                    )?;
                }
                Lua_::Table(table)
            }
        })
    }

    /// Reads a Lua value.
    ///
    /// Three kinds of value have no representation here and are refused rather than
    /// silently flattened: functions, threads and userdata. A plugin returning one
    /// has written for an in-process Rust host, and saying so is more useful than
    /// handing the caller a `nil` it will misread.
    ///
    /// Lua strings are byte strings, so one that is not UTF-8 is also refused. The
    /// alternative is lossy replacement, which corrupts the value on its way through
    /// a boundary whose whole job is to carry it faithfully.
    pub fn from_lua(value: &Lua_) -> mlua::Result<Value> {
        Value::from_lua_at(value, 0)
    }

    fn from_lua_at(value: &Lua_, depth: usize) -> mlua::Result<Value> {
        if depth > MAX_DEPTH {
            return Err(mlua::Error::RuntimeError(format!(
                "table nests deeper than {MAX_DEPTH} levels, or contains itself"
            )));
        }
        Ok(match value {
            Lua_::Nil => Value::Nil,
            Lua_::Boolean(value) => Value::Bool(*value),
            Lua_::Integer(value) => Value::Int(*value),
            Lua_::Number(value) => Value::Float(*value),
            Lua_::String(value) => Value::Str(value.to_str()?.to_string()),
            Lua_::Table(table) => table_to_value(table, depth)?,
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
/// Lua has one table type doing both jobs, so the distinction has to be inferred:
/// keys of exactly `1..=n` make a list, anything else a map. Two consequences worth
/// knowing, both documented in `docs/bindings.md`:
///
/// * An empty table becomes an empty [`Value::List`]. Lua cannot tell the two apart,
///   so neither can this.
/// * A sparse array — `{[1] = "a", [3] = "c"}` — becomes a map with the keys `"1"`
///   and `"3"`, because it is not a sequence and pretending otherwise would drop the
///   third element.
fn table_to_value(table: &mlua::Table, depth: usize) -> mlua::Result<Value> {
    let next = depth.saturating_add(1);

    let length = table.raw_len();
    // Starts true so an empty table settles as an empty list: `pairs.len() == length`
    // holds at zero. Any key outside `1..=length` clears it below.
    let mut is_sequence = true;
    let mut pairs = Vec::new();

    for pair in table.pairs::<Lua_, Lua_>() {
        let (key, value) = pair?;
        // A sequence's keys are exactly the integers `1..=raw_len`. Anything outside
        // that — a string key, a zero, a hole's neighbour — settles it as a map.
        if is_sequence {
            let within = match &key {
                Lua_::Integer(index) => *index >= 1 && (*index as usize) <= length,
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
            if let Lua_::Integer(index) = key {
                // `within` above already bounded this, so the subtraction and the
                // index are both in range.
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
fn key_to_string(key: &Lua_) -> mlua::Result<String> {
    Ok(match key {
        Lua_::String(key) => key.to_str()?.to_string(),
        Lua_::Integer(key) => key.to_string(),
        Lua_::Number(key) => key.to_string(),
        Lua_::Boolean(key) => key.to_string(),
        other => {
            return Err(mlua::Error::RuntimeError(format!(
                "a table keyed by {} cannot cross a language boundary",
                other.type_name()
            )));
        }
    })
}

/// Converts a value into the TOML a [`crate::Decision::GrantWith`] narrows with.
///
/// Capability parameters are TOML on the manifest side, so a foreign policy that
/// narrows a grant has to land back in TOML. TOML has no null, so a [`Value::Nil`]
/// drops the key rather than inventing one.
pub(crate) fn value_to_toml(value: &Value) -> Result<toml::Value, String> {
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
pub(crate) fn toml_to_value(value: &toml::Value) -> Value {
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
pub(crate) fn table_to_map(table: &toml::Table) -> Value {
    Value::Map(table.iter().map(|(key, value)| (key.clone(), toml_to_value(value))).collect())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn lua() -> Lua {
        Lua::new()
    }

    fn round_trip(source: &str) -> Value {
        let lua = lua();
        let value: Lua_ = lua.load(source).eval().expect("evaluating the chunk");
        Value::from_lua(&value).expect("converting to a Value")
    }

    #[test]
    fn scalars_survive_the_round_trip() {
        assert_eq!(round_trip("return nil"), Value::Nil);
        assert_eq!(round_trip("return true"), Value::Bool(true));
        assert_eq!(round_trip("return 7"), Value::Int(7));
        assert_eq!(round_trip("return 7.5"), Value::Float(7.5));
        assert_eq!(round_trip("return 'hi'"), Value::Str("hi".to_string()));
    }

    #[test]
    fn a_contiguous_table_is_a_list() {
        assert_eq!(
            round_trip("return {10, 20, 30}"),
            Value::List(vec![Value::Int(10), Value::Int(20), Value::Int(30)])
        );
    }

    #[test]
    fn a_keyed_table_is_a_map() {
        let Value::Map(entries) = round_trip("return {a = 1, b = 2}") else {
            panic!("expected a map");
        };
        assert_eq!(entries.get("a"), Some(&Value::Int(1)));
        assert_eq!(entries.get("b"), Some(&Value::Int(2)));
    }

    #[test]
    fn a_sparse_array_becomes_a_map_rather_than_losing_elements() {
        let Value::Map(entries) = round_trip("return {[1] = 'a', [3] = 'c'}") else {
            panic!("expected a map");
        };
        assert_eq!(entries.get("1"), Some(&Value::Str("a".to_string())));
        assert_eq!(entries.get("3"), Some(&Value::Str("c".to_string())));
    }

    #[test]
    fn an_empty_table_is_an_empty_list() {
        assert_eq!(round_trip("return {}"), Value::List(Vec::new()));
    }

    #[test]
    fn a_mixed_table_keeps_every_entry() {
        let Value::Map(entries) = round_trip("return {1, 2, name = 'x'}") else {
            panic!("expected a map");
        };
        assert_eq!(entries.len(), 3);
        assert_eq!(entries.get("name"), Some(&Value::Str("x".to_string())));
    }

    #[test]
    fn a_self_referential_table_is_refused_rather_than_overflowing() {
        let lua = lua();
        let value: Lua_ = lua
            .load("local t = {}; t.self = t; return t")
            .eval()
            .expect("evaluating the chunk");
        let err = Value::from_lua(&value).expect_err("a cycle must not convert");
        assert!(err.to_string().contains("contains itself"), "{err}");
    }

    #[test]
    fn a_function_cannot_cross_the_boundary() {
        let lua = lua();
        let value: Lua_ = lua.load("return function() end").eval().expect("chunk");
        let err = Value::from_lua(&value).expect_err("a function must not convert");
        assert!(err.to_string().contains("function"), "{err}");
    }

    #[test]
    fn values_make_the_lua_they_describe() {
        let lua = lua();
        let value = Value::Map(BTreeMap::from([
            ("n".to_string(), Value::Int(3)),
            ("xs".to_string(), Value::List(vec![Value::Str("a".to_string())])),
        ]));
        let built = value.to_lua(&lua).expect("building the Lua value");
        assert_eq!(Value::from_lua(&built).expect("reading it back"), value);
    }

    #[test]
    fn a_grant_cannot_carry_a_null() {
        assert!(value_to_toml(&Value::Nil).is_err());
    }
}

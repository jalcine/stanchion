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
    /// A Lua string. Must be UTF-8.
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

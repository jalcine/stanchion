//! Ruby objects in, [`Value`]s out, and back.

use magnus::value::ReprValue;
use magnus::{Error, Float, Integer, RArray, RHash, RString, Ruby, Symbol, Value as Rb};

use stanchion_ffi::Value;

/// How deep a Ruby structure may nest before conversion gives up.
///
/// Matches the limit on the Lua side, and for the same reason: an array that
/// contains itself is easy to build and would otherwise overflow the stack.
const MAX_DEPTH: usize = 64;

fn type_error(ruby: &Ruby, message: String) -> Error {
    Error::new(ruby.exception_type_error(), message)
}

/// Converts a Ruby object into the value a plugin will see.
pub fn from_ruby(ruby: &Ruby, value: Rb) -> Result<Value, Error> {
    from_ruby_at(ruby, value, 0)
}

fn from_ruby_at(ruby: &Ruby, value: Rb, depth: usize) -> Result<Value, Error> {
    if depth > MAX_DEPTH {
        return Err(type_error(
            ruby,
            format!("value nests deeper than {MAX_DEPTH} levels, or contains itself"),
        ));
    }
    let next = depth.saturating_add(1);

    if value.is_nil() {
        return Ok(Value::Nil);
    }
    // Compared against the singletons rather than asked to convert: every object
    // other than `nil` and `false` is truthy in Ruby, so a conversion would turn
    // every string into `true`.
    if value.equal(ruby.qtrue())? {
        return Ok(Value::Bool(true));
    }
    if value.equal(ruby.qfalse())? {
        return Ok(Value::Bool(false));
    }
    if let Some(value) = Integer::from_value(value) {
        return Ok(Value::Int(value.to_i64()?));
    }
    if let Some(value) = Float::from_value(value) {
        return Ok(Value::Float(value.to_f64()));
    }
    if let Some(value) = RString::from_value(value) {
        return Ok(Value::Str(value.to_string()?));
    }
    // A symbol is the natural way to write a short constant in Ruby, and Lua has
    // nothing else to call it, so it arrives as a string.
    if let Some(value) = Symbol::from_value(value) {
        return Ok(Value::Str(value.name()?.to_string()));
    }
    if let Some(array) = RArray::from_value(value) {
        let mut items = Vec::with_capacity(array.len());
        for item in array.into_iter() {
            items.push(from_ruby_at(ruby, item, next)?);
        }
        return Ok(Value::List(items));
    }
    if let Some(hash) = RHash::from_value(value) {
        let mut entries = std::collections::BTreeMap::new();
        let pairs: Vec<(Rb, Rb)> = {
            let mut collected = Vec::new();
            hash.foreach(|key: Rb, entry: Rb| {
                collected.push((key, entry));
                Ok(magnus::r_hash::ForEach::Continue)
            })?;
            collected
        };
        for (key, entry) in pairs {
            entries.insert(key_to_string(ruby, key)?, from_ruby_at(ruby, entry, next)?);
        }
        return Ok(Value::Map(entries));
    }

    Err(type_error(
        ruby,
        format!(
            "cannot pass a {} to a Lua plugin; use nil, true, false, Integer, Float, \
             String, Symbol, Array or Hash",
            value.class().inspect(),
        ),
    ))
}

/// Renders a hash key as a string, since Lua table keys arrive as strings.
fn key_to_string(ruby: &Ruby, key: Rb) -> Result<String, Error> {
    if let Some(key) = Symbol::from_value(key) {
        return Ok(key.name()?.to_string());
    }
    if let Some(key) = RString::from_value(key) {
        return key.to_string();
    }
    if let Some(key) = Integer::from_value(key) {
        return Ok(key.to_i64()?.to_string());
    }
    Err(type_error(
        ruby,
        format!("a Hash keyed by {} cannot cross into Lua", key.class().inspect()),
    ))
}

/// Converts a plugin's value into the Ruby object a caller gets back.
pub fn to_ruby(ruby: &Ruby, value: &Value) -> Result<Rb, Error> {
    Ok(match value {
        Value::Nil => ruby.qnil().as_value(),
        Value::Bool(true) => ruby.qtrue().as_value(),
        Value::Bool(false) => ruby.qfalse().as_value(),
        Value::Int(value) => ruby.integer_from_i64(*value).as_value(),
        Value::Float(value) => ruby.float_from_f64(*value).as_value(),
        Value::Str(value) => ruby.str_new(value).as_value(),
        Value::List(items) => {
            let array = ruby.ary_new_capa(items.len());
            for item in items {
                array.push(to_ruby(ruby, item)?)?;
            }
            array.as_value()
        }
        Value::Map(entries) => {
            let hash = ruby.hash_new();
            for (key, entry) in entries {
                hash.aset(ruby.str_new(key), to_ruby(ruby, entry)?)?;
            }
            hash.as_value()
        }
        Value::Function => ruby.qnil().as_value(),
    })
}

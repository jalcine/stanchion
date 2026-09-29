//! Godot `Variant`s in, [`Value`]s out, and back.
//!
//! Everything that decides *behaviour* lives in `stanchion-ffi`; this file only
//! carries data across the boundary, the same shape the Python and Ruby shims carry
//! it, so the bindings cannot drift apart.

use godot::builtin::{Array, GString, VarArray, VarDictionary, Variant};
use godot::meta::ToGodot;
use godot::sys::VariantType;

use stanchion_ffi::Value;

/// How deep a Godot structure may nest before conversion gives up.
///
/// Matches the limit on the Lua side, and for the same reason: an array that contains
/// itself is easy to build and would otherwise overflow the stack.
const MAX_DEPTH: usize = 64;

/// Converts a Godot value into the value a plugin will see.
pub fn from_variant(variant: &Variant) -> Result<Value, String> {
    from_variant_at(variant, 0)
}

fn from_variant_at(variant: &Variant, depth: usize) -> Result<Value, String> {
    if depth > MAX_DEPTH {
        return Err(format!(
            "value nests deeper than {MAX_DEPTH} levels, or contains itself"
        ));
    }
    let next = depth.saturating_add(1);

    match variant.get_type() {
        VariantType::NIL => Ok(Value::Nil),
        VariantType::BOOL => convert(variant.try_to::<bool>()).map(Value::Bool),
        VariantType::INT => convert(variant.try_to::<i64>()).map(Value::Int),
        VariantType::FLOAT => convert(variant.try_to::<f64>()).map(Value::Float),
        // `String`, `StringName` and `NodePath` all read back as text; a plugin only
        // ever sees UTF-8 strings, so they collapse to one variant here.
        VariantType::STRING | VariantType::STRING_NAME | VariantType::NODE_PATH => {
            convert(variant.try_to::<GString>()).map(|text| Value::Str(text.to_string()))
        }
        VariantType::ARRAY => {
            let items = array_elements(variant)?;
            let mut values = Vec::with_capacity(items.len());
            for item in &items {
                values.push(from_variant_at(item, next)?);
            }
            Ok(Value::List(values))
        }
        VariantType::DICTIONARY => {
            let dictionary = convert(variant.try_to::<VarDictionary>())?;
            let mut entries = std::collections::BTreeMap::new();
            for (key, value) in dictionary.iter_shared() {
                // Lua table keys arrive as strings, so this is the same shape in
                // reverse: whatever the key is, it is stringified.
                let key = key.try_to::<GString>().unwrap_or_else(|_| key.stringify());
                entries.insert(key.to_string(), from_variant_at(&value, next)?);
            }
            Ok(Value::Map(entries))
        }
        other => Err(format!(
            "cannot pass a {other:?} to a Lua plugin; use null, bool, int, float, \
             String, Array or Dictionary"
        )),
    }
}

/// Converts a plugin's value into the Godot value a caller gets back.
pub fn to_variant(value: &Value) -> Variant {
    match value {
        Value::Nil => Variant::nil(),
        Value::Bool(value) => value.to_variant(),
        Value::Int(value) => value.to_variant(),
        Value::Float(value) => value.to_variant(),
        Value::Str(value) => value.to_variant(),
        Value::List(items) => {
            let mut array = VarArray::new();
            for item in items {
                array.push(&to_variant(item));
            }
            array.to_variant()
        }
        Value::Map(entries) => {
            let mut dictionary = VarDictionary::new();
            for (key, entry) in entries {
                let key = key.to_variant();
                dictionary.set(&key, &to_variant(entry));
            }
            dictionary.to_variant()
        }
        // A Lua function cannot cross the language boundary, and never leaves the Lua
        // side in the first place — the same as every other cross-language binding.
        Value::Function => Variant::nil(),
    }
}

/// Reads a Godot Array's elements as `Variant`s, whether the array is untyped or a
/// typed `Array[T]`.
///
/// GDScript hands out typed arrays freely — `Array[String]`, `Array[int]` — and a
/// typed array does not convert to an untyped [`VarArray`]. So untyped is tried first
/// (the common case), then the primitive element types a plugin's data is built from.
fn array_elements(variant: &Variant) -> Result<Vec<Variant>, String> {
    if let Ok(array) = variant.try_to::<VarArray>() {
        return Ok(array.iter_shared().collect());
    }
    if let Ok(array) = variant.try_to::<Array<GString>>() {
        return Ok(array.iter_shared().map(|item| item.to_variant()).collect());
    }
    if let Ok(array) = variant.try_to::<Array<i64>>() {
        return Ok(array.iter_shared().map(|item| item.to_variant()).collect());
    }
    if let Ok(array) = variant.try_to::<Array<f64>>() {
        return Ok(array.iter_shared().map(|item| item.to_variant()).collect());
    }
    if let Ok(array) = variant.try_to::<Array<bool>>() {
        return Ok(array.iter_shared().map(|item| item.to_variant()).collect());
    }
    if let Ok(array) = variant.try_to::<Array<VarDictionary>>() {
        return Ok(array.iter_shared().map(|item| item.to_variant()).collect());
    }
    Err(
        "cannot read this typed Array; use an untyped Array or one of String, \
         int, float, bool or Dictionary elements"
            .to_string(),
    )
}

/// Turns a Godot conversion error into the flat string the whole binding reports.
fn convert<T>(result: Result<T, godot::meta::error::ConvertError>) -> Result<T, String> {
    result.map_err(|error| error.to_string())
}

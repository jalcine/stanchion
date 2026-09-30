//! Tests for the [`Value`] type and its conversions.

use stanchion_abi::value::{Value, table_to_map, toml_to_value, value_to_toml};
use std::collections::BTreeMap;

/// Fallible tests read better than `unwrap` and keep the workspace deny on
/// panicking helpers intact.
type Boxed = Box<dyn std::error::Error + Send + Sync>;
type Fallible<T> = std::result::Result<T, Boxed>;

#[test]
fn nil_round_trips_through_toml() -> Fallible<()> {
    // Nil cannot be represented in TOML, so value_to_toml should error
    let err = value_to_toml(&Value::Nil)
        .err()
        .ok_or("nil must not convert to TOML")?;
    assert!(err.contains("TOML has no null"));

    // But toml_to_value and table_to_map don't produce Nil from TOML
    let table = toml::Table::new();
    let val = table_to_map(&table);
    assert_eq!(val, Value::Map(BTreeMap::new()));
    Ok(())
}

#[test]
fn bool_round_trips_through_toml() -> Fallible<()> {
    for b in [true, false] {
        let val = Value::Bool(b);
        let toml = value_to_toml(&val)?;
        assert_eq!(toml, toml::Value::Boolean(b));

        let back = toml_to_value(&toml);
        assert_eq!(back, val);
    }
    Ok(())
}

#[test]
fn int_round_trips_through_toml() -> Fallible<()> {
    for i in [-9, 0, 7, i64::MAX, i64::MIN] {
        let val = Value::Int(i);
        let toml = value_to_toml(&val)?;
        assert_eq!(toml, toml::Value::Integer(i));

        let back = toml_to_value(&toml);
        assert_eq!(back, val);
    }
    Ok(())
}

#[test]
fn float_round_trips_through_toml() -> Fallible<()> {
    for f in [-9.0, 0.0, 1.5, f64::MAX, f64::MIN] {
        let val = Value::Float(f);
        let toml = value_to_toml(&val)?;
        assert_eq!(toml, toml::Value::Float(f));

        let back = toml_to_value(&toml);
        assert_eq!(back, val);
    }
    // NaN and infinity round-trip through toml representation, not through PartialEq
    let val = Value::Float(f64::NAN);
    let toml = value_to_toml(&val)?;
    let back = toml_to_value(&toml);
    // f64::NAN is a float that round-trips (NaN != NaN by design, so skip PartialEq check)
    assert!(matches!(back, Value::Float(_)));

    // f64::INFINITY and f64::NEG_INFINITY are valid floats that round-trip
    let pos_inf = Value::Float(f64::INFINITY);
    let neg_inf = Value::Float(f64::NEG_INFINITY);
    let pos_toml = value_to_toml(&pos_inf)?;
    let neg_toml = value_to_toml(&neg_inf)?;
    assert!(matches!(pos_toml, toml::Value::Float(_)));
    assert!(matches!(neg_toml, toml::Value::Float(_)));
    assert!(matches!(toml_to_value(&pos_toml), Value::Float(_)));
    assert!(matches!(toml_to_value(&neg_toml), Value::Float(_)));
    Ok(())
}

#[test]
fn string_round_trips_through_toml() -> Fallible<()> {
    for s in ["", "hello", "🦀", "multi\nline"] {
        let val = Value::Str(s.to_string());
        let toml = value_to_toml(&val)?;
        assert_eq!(toml, toml::Value::String(s.to_string()));

        let back = toml_to_value(&toml);
        assert_eq!(back, val);
    }
    Ok(())
}

#[test]
fn list_round_trips_through_toml() -> Fallible<()> {
    let val = Value::List(vec![
        Value::Int(1),
        Value::Int(2),
        Value::Str("three".to_string()),
    ]);
    let toml = value_to_toml(&val)?;
    assert_eq!(
        toml,
        toml::Value::Array(vec![
            toml::Value::Integer(1),
            toml::Value::Integer(2),
            toml::Value::String("three".to_string()),
        ])
    );

    let back = toml_to_value(&toml);
    assert_eq!(back, val);
    Ok(())
}

#[test]
fn map_round_trips_through_toml() -> Fallible<()> {
    let mut entries = BTreeMap::new();
    entries.insert("a".to_string(), Value::Int(1));
    entries.insert("b".to_string(), Value::Bool(true));
    let val = Value::Map(entries.clone());

    let toml = value_to_toml(&val)?;
    let back = toml_to_value(&toml);
    assert_eq!(back, val);
    Ok(())
}

#[test]
fn nested_structures_round_trip() -> Fallible<()> {
    let mut inner = BTreeMap::new();
    inner.insert("nested".to_string(), Value::Int(42));
    let val = Value::Map(BTreeMap::from([
        (
            "list".to_string(),
            Value::List(vec![Value::Int(1), Value::Int(2)]),
        ),
        ("map".to_string(), Value::Map(inner)),
    ]));

    let toml = value_to_toml(&val)?;
    let back = toml_to_value(&toml);
    assert_eq!(back, val);
    Ok(())
}

#[test]
fn map_with_nil_drops_key_in_toml() -> Fallible<()> {
    let mut entries = BTreeMap::new();
    entries.insert("keep".to_string(), Value::Int(1));
    entries.insert("drop".to_string(), Value::Nil);
    let val = Value::Map(entries);

    let toml = value_to_toml(&val)?;
    let table = toml.as_table().ok_or("a map must become a table")?;
    assert!(table.contains_key("keep"));
    assert!(!table.contains_key("drop"));
    Ok(())
}

#[test]
fn empty_list_is_list_not_map() -> Fallible<()> {
    let val = Value::List(Vec::new());
    let toml = value_to_toml(&val)?;
    assert_eq!(toml, toml::Value::Array(Vec::new()));
    Ok(())
}

#[test]
fn empty_map_is_map_not_list() -> Fallible<()> {
    let val = Value::Map(BTreeMap::new());
    let toml = value_to_toml(&val)?;
    assert_eq!(toml, toml::Value::Table(toml::Table::new()));
    Ok(())
}

#[test]
fn toml_datetime_becomes_string() -> Fallible<()> {
    let dt = "2024-01-15T10:30:00Z".parse::<toml::value::Datetime>()?;
    let toml_val = toml::Value::Datetime(dt);
    let val = toml_to_value(&toml_val);
    assert!(matches!(val, Value::Str(_)));
    Ok(())
}

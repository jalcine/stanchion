//! Tests for the internal [`__private`] module functions.

use mlua::{Lua, Value as LuaValue};
use stanchion_core::__private::{
    expect_handle, expect_table, optional_function, require_functions,
    require_handle_functions,
};

#[test]
fn expect_handle_with_table_succeeds() {
    let lua = Lua::new();
    let table = lua.create_table().unwrap();
    let value = LuaValue::Table(table);
    let handle = expect_handle(value, "test_handle").unwrap();
    assert!(handle.as_table().is_some());
}

#[test]
fn expect_handle_with_userdata_succeeds() {
    let lua = Lua::new();
    let table = lua.create_table().unwrap();
    let value = LuaValue::Table(table);
    let handle = expect_handle(value, "test_handle").unwrap();
    assert!(handle.as_table().is_some());
}

#[test]
fn expect_handle_with_integer_fails() {
    let lua = Lua::new();
    let value = LuaValue::Integer(42);
    let err = expect_handle(value, "test_handle").unwrap_err();
    assert!(err.to_string().contains("expected a table or userdata"));
}

#[test]
fn expect_table_with_table_succeeds() {
    let lua = Lua::new();
    let table = lua.create_table().unwrap();
    let value = LuaValue::Table(table);
    let result = expect_table(value, "test_table");
    assert!(result.is_ok());
}

#[test]
fn expect_table_with_string_fails() {
    let lua = Lua::new();
    let value = LuaValue::String(lua.create_string("hello").unwrap());
    let err = expect_table(value, "test_table").unwrap_err();
    assert!(err.to_string().contains("expected a table"));
}

#[test]
fn optional_function_finds_existing() {
    let lua = Lua::new();
    let table = lua.create_table().unwrap();
    table.set("greet", lua.create_function(|_, _| Ok(())).unwrap()).unwrap();
    let handle = stanchion_core::LuaHandle::Table(table);
    let result = optional_function(&handle, "greet").unwrap();
    assert!(result.is_some());
}

#[test]
fn optional_function_returns_none_for_missing() {
    let lua = Lua::new();
    let table = lua.create_table().unwrap();
    let handle = stanchion_core::LuaHandle::Table(table);
    let result = optional_function(&handle, "missing").unwrap();
    assert!(result.is_none());
}

#[test]
fn require_functions_succeeds_for_valid_table() {
    let lua = Lua::new();
    let table = lua.create_table().unwrap();
    table.set("greet", lua.create_function(|_, _| Ok(())).unwrap()).unwrap();
    let result = require_functions(&table, "test", &["greet"]);
    assert!(result.is_ok());
}

#[test]
fn require_functions_fails_for_missing_function() {
    let lua = Lua::new();
    let table = lua.create_table().unwrap();
    let err = require_functions(&table, "test", &["greet"]).unwrap_err();
    assert!(err.to_string().contains("missing required function"));
}

#[test]
fn require_functions_fails_for_non_function() {
    let lua = Lua::new();
    let table = lua.create_table().unwrap();
    table.set("greet", 42).unwrap();
    let err = require_functions(&table, "test", &["greet"]).unwrap_err();
    assert!(err.to_string().contains("must be a function"));
}

#[test]
fn require_handle_functions_succeeds_for_valid_handle() {
    let lua = Lua::new();
    let table = lua.create_table().unwrap();
    table.set("greet", lua.create_function(|_, _| Ok(())).unwrap()).unwrap();
    let handle = stanchion_core::LuaHandle::Table(table);
    let result = require_handle_functions(&handle, "test", &["greet"]);
    assert!(result.is_ok());
}

#[test]
fn require_handle_functions_fails_for_non_function() {
    let lua = Lua::new();
    let table = lua.create_table().unwrap();
    table.set("greet", 42).unwrap();
    let handle = stanchion_core::LuaHandle::Table(table);
    let err = require_handle_functions(&handle, "test", &["greet"]).unwrap_err();
    assert!(err.to_string().contains("must be a function"));
}
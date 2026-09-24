//! Tests for [`LuaHandle`] and related types.

use mlua::{FromLua, Lua, Value as LuaValue};
use stanchion_core::LuaHandle;

#[test]
fn table_handle_type_name() {
    let lua = Lua::new();
    let table = lua.create_table().unwrap();
    let handle = LuaHandle::Table(table);
    assert_eq!(handle.type_name(), "table");
}

#[test]
fn userdata_handle_type_name() {
    let lua = Lua::new();
    let table = lua.create_table().unwrap();
    let handle: LuaHandle = table.into();
    assert!(matches!(handle, LuaHandle::Table(_)));
}

#[test]
fn table_to_value() {
    let lua = Lua::new();
    let table = lua.create_table().unwrap();
    table.set("key", "value").unwrap();
    let handle = LuaHandle::Table(table);
    let value = handle.to_value();
    assert!(matches!(value, LuaValue::Table(_)));
}

#[test]
fn from_table_works() {
    let lua = Lua::new();
    let table = lua.create_table().unwrap();
    let handle: LuaHandle = table.into();
    assert!(matches!(handle, LuaHandle::Table(_)));
}

#[test]
fn from_lua_table() {
    let lua = Lua::new();
    let table = lua.create_table().unwrap();
    let value = LuaValue::Table(table);
    let handle = LuaHandle::from_lua(value, &lua).unwrap();
    assert!(matches!(handle, LuaHandle::Table(_)));
}

#[test]
fn from_lua_non_table_fails() {
    let lua = Lua::new();
    let value = LuaValue::Integer(42);
    let result = LuaHandle::from_lua(value, &lua);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("expected a table or userdata"));
}

#[test]
fn as_table_returns_some_for_table() {
    let lua = Lua::new();
    let table = lua.create_table().unwrap();
    let handle = LuaHandle::Table(table);
    assert!(handle.as_table().is_some());
}

#[test]
fn as_userdata_returns_none_for_table() {
    let lua = Lua::new();
    let table = lua.create_table().unwrap();
    let handle = LuaHandle::Table(table);
    assert!(handle.as_userdata().is_none());
}

#[test]
fn get_reads_key_from_table() {
    let lua = Lua::new();
    let table = lua.create_table().unwrap();
    table.set("name", "value").unwrap();
    let handle = LuaHandle::Table(table);
    let value: String = handle.get("name").unwrap();
    assert_eq!(value, "value");
}

#[test]
fn set_writes_key_to_table() {
    let lua = Lua::new();
    let table = lua.create_table().unwrap();
    let handle = LuaHandle::Table(table);
    handle.set("name", "value").unwrap();
    let value: String = handle.get("name").unwrap();
    assert_eq!(value, "value");
}

#[test]
fn call_method_invokes_table_method() {
    let lua = Lua::new();
    let table = lua.create_table().unwrap();
    // call_method passes the table as the implicit "self" arg (Lua's obj:method),
    // so the closure must accept (self_table, name) as a 2-tuple of args.
    let func = lua.create_function(|_lua, (_table, name): (mlua::Value, String)| Ok(format!("hello, {name}"))).unwrap();
    table.set("greet", func).unwrap();
    let handle = LuaHandle::Table(table);
    let result: String = handle.call_method("greet", "world").unwrap();
    assert_eq!(result, "hello, world");
}

#[test]
fn call_function_invokes_table_function() {
    let lua = Lua::new();
    let table = lua.create_table().unwrap();
    let func = lua.create_function(|_, (a, b): (i64, i64)| Ok(a + b)).unwrap();
    table.set("add", func).unwrap();
    let handle = LuaHandle::Table(table);
    let result: i64 = handle.call_function("add", (2, 3)).unwrap();
    assert_eq!(result, 5);
}

#[test]
fn handle_is_clone() {
    let lua = Lua::new();
    let table = lua.create_table().unwrap();
    table.set("name", "value").unwrap();
    let handle = LuaHandle::Table(table);
    let cloned = handle.clone();
    assert!(cloned.as_table().is_some());
}

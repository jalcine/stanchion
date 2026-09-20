//! Classes backed by userdata rather than a Lua table.
#![cfg(feature = "registry")]

use stanchion::mlua::{AnyUserData, Lua, Result, UserData, UserDataMethods};
use stanchion::{LuaClass, LuaHandle, LuaObject, load_class, lua_class};

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

/// Rust-owned state, exposed to Lua as userdata.
struct Counter {
    count: i64,
}

impl UserData for Counter {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method_mut("bump", |_, this, amount: i64| {
            this.count = this.count.saturating_add(amount);
            Ok(this.count)
        });
        methods.add_method("value", |_, this, ()| Ok(this.count));
        methods.add_method("label", |_, _, ()| Ok("counter"));
    }
}

/// The same trait shape as a table-backed class; nothing here says "userdata".
#[lua_class]
pub trait Tally {
    fn new(start: i64) -> Result<Self>;
    fn bump(&self, amount: i64) -> Result<i64>;
    fn value(&self) -> Result<i64>;
    #[lua(optional)]
    fn reset(&self) -> Result<Option<()>>;
}

const SOURCE: &str = r#"
local Tally = {}
-- `new` is a Rust function returning userdata, so instances are not tables.
Tally.new = make_counter
return Tally
"#;

fn lua_with_constructor() -> Result<Lua> {
    let lua = Lua::new();
    let make =
        lua.create_function(|lua, start: i64| lua.create_userdata(Counter { count: start }))?;
    lua.globals().set("make_counter", make)?;
    Ok(lua)
}

#[test]
fn a_userdata_instance_satisfies_a_lua_class_contract() -> TestResult {
    let lua = lua_with_constructor()?;
    let class: TallyClass = load_class(&lua, SOURCE, "tally.lua")?;

    let tally = class.new(10)?;
    assert_eq!(tally.value()?, 10);
    assert_eq!(tally.bump(5)?, 15);
    assert_eq!(tally.bump(-3)?, 12);
    assert_eq!(tally.value()?, 12);
    Ok(())
}

#[test]
fn validation_reaches_methods_on_a_userdata_metatable() -> TestResult {
    let lua = lua_with_constructor()?;
    let class: TallyClass = load_class(&lua, SOURCE, "tally.lua")?;
    let tally = class.new(0)?;

    // `bump` and `value` are required and live on the userdata's metatable, not on
    // the value itself; validation resolved them through `__index`.
    assert_eq!(
        <TallyHandle as LuaObject>::required_methods(),
        ["bump", "value"]
    );
    // The optional one is absent, which is allowed.
    assert_eq!(tally.reset()?, None);
    Ok(())
}

#[test]
fn a_handle_reports_what_backs_it() -> TestResult {
    let lua = lua_with_constructor()?;
    let class: TallyClass = load_class(&lua, SOURCE, "tally.lua")?;
    let tally = class.new(1)?;

    // The class is a table; the instance is userdata.
    assert_eq!(class.handle().type_name(), "table");
    assert!(class.handle().as_table().is_some());

    assert_eq!(tally.handle().type_name(), "userdata");
    assert!(tally.handle().as_table().is_none());
    assert!(tally.handle().as_userdata().is_some());
    Ok(())
}

#[test]
fn a_userdata_instance_is_rejected_when_it_lacks_a_required_method() -> TestResult {
    struct Bare;
    impl UserData for Bare {}

    let lua = Lua::new();
    let make = lua.create_function(|lua, ()| lua.create_userdata(Bare))?;
    lua.globals().set("make_counter", make)?;

    let class: TallyClass = load_class(&lua, SOURCE, "tally.lua")?;
    let Err(error) = class.new(0) else {
        return Err("userdata without `bump` should be rejected".into());
    };
    assert!(
        error
            .to_string()
            .contains("missing required function `bump`"),
        "got: {error}"
    );
    // The error names what actually backed the value.
    assert!(error.to_string().contains("userdata"), "got: {error}");
    Ok(())
}

#[test]
fn a_handle_converts_from_either_lua_shape() -> TestResult {
    let lua = lua_with_constructor()?;

    let table: LuaHandle = lua.load("return {}").eval()?;
    assert_eq!(table.type_name(), "table");

    let data: LuaHandle = lua.load("return make_counter(3)").eval()?;
    assert_eq!(data.type_name(), "userdata");
    assert_eq!(data.call_method::<i64>("value", ())?, 3);

    let refused = lua.load("return 42").eval::<LuaHandle>();
    assert!(refused.is_err(), "a number is neither a table nor userdata");

    let _: AnyUserData = data.as_userdata().cloned().ok_or("expected userdata")?;
    Ok(())
}

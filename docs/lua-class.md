# Typed class bindings

How `#[lua_class]` turns a Rust trait into a Lua class binding: what it
generates, the attributes it accepts, and what it validates.

# What the macro generates

| Item | Role |
| --- | --- |
| `trait Greeter` | rewritten to hold only `&self` methods, so it stays dyn-compatible |
| `GreeterClass` | handle to the class table; receiverless trait fns become inherent methods on it |
| `GreeterHandle` | instance handle; `impl Greeter for GreeterHandle` delegates to the Lua object |
| `FromLua` for both | validates the contract at the load boundary |
| `LuaClass` / `LuaObject` impls | `CLASS_NAME`, `required_functions()`, `required_methods()`, table access |

Because the trait survives as a plain Rust trait, Lua-backed and native implementations
mix freely:

```rust
let registry: Vec<Box<dyn Greeter>> = vec![Box::new(handle), Box::new(NativeGreeter)];
```


# Method attributes

| Attribute | Effect |
| --- | --- |
| *(none)*, with `&self` | `obj:name(..)` — colon call, object passed implicitly |
| *(none)*, no receiver | `Class.name(..)` — moved onto `{Trait}Class` |
| `#[lua(function)]` | `obj.name(..)` — dot call on an instance |
| `#[lua(field)]` | field read (no args) or write (exactly one arg) |
| `#[lua(optional)]` | missing key yields `Ok(None)`; return type must be `Result<Option<T>>` |
| `#[lua(name = "..")]` | override the Lua key |

`#[lua_class(class = "..", handle = "..", name = "..")]` renames the generated types and
the class name used in error messages. A `set_`-prefixed field method defaults to the
key without the prefix (`set_greeting` → `greeting`).

Lookups go through `ObjectLike`, which honours `__index`, so methods inherited from a
base class resolve and validate correctly.


# Validation

`FromLua` checks that every required key resolves to a function before handing back a
handle, so a malformed plugin fails at load with a typed error rather than
`attempt to call a nil value` mid-request:

```
error converting Lua table to GreeterHandle (missing required function `greet`)
```

`#[lua(optional)]` methods and fields are exempt.

`#[cfg]` on a trait method is honoured end to end: the method is dropped from the trait,
from the impl, and from the required-key set, so a class compiled without it still loads.
That is why the required keys are `required_methods()` / `required_functions()` rather
than consts — array elements cannot carry `#[cfg]`.

## Tables or userdata

A handle wraps either a Lua table or userdata. Nothing in the trait says which, and the
same contract binds both:

```rust
#[lua_class]
pub trait Tally {
    fn new(start: i64) -> Result<Self>;
    fn bump(&self, amount: i64) -> Result<i64>;
}
```

```lua
local Tally = {}
Tally.new = make_counter   -- a Rust function returning userdata
return Tally
```

`mlua`'s `ObjectLike` covers both shapes but is sealed, so `LuaHandle` re-dispatches
the operations generated code needs. Validation resolves required methods through
`__index`, which reaches methods on a userdata metatable; userdata carrying no methods
at all has no `__index` and raises when probed, so that is reported as the missing
method rather than as a Lua error.

`LuaObject::handle()` returns the `LuaHandle`; `table()` returns `Option<&Table>` for
the features that genuinely need a table, such as the registry's `exports` proxies.

---

[← Documentation index](../README.md#documentation)

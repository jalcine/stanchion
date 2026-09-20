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

---

[← Documentation index](../README.md#documentation)

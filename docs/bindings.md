# Language bindings

Using stanchion from Python, Kotlin, Swift, Ruby and JavaScript.

The Rust API's `Registry<C: LuaClass>` is generic over a contract `#[lua_class]` derives
from a trait at compile time. No foreign language can supply one, so the bindings take
the same path [out-of-process hosting](remote.md) already takes: plugins load as
`DynClass` — any table with a constructor — and methods resolve by name at call time.

`stanchion-ffi` is the seam every binding sits on. It carries no binding dependencies
of its own, so nothing `pyo3` or `uniffi` finds convenient can leak into the shape the
other four see.

```
                        ┌─ bindings/python  (pyo3 + maturin)     ── shipped
                        ├─ bindings/uniffi  (Kotlin, Swift)      ── Kotlin runs; packaging pending
stanchion-ffi ──────────┼─ bindings/ruby    (magnus)             ── planned
  Stanchion             └─ bindings/node    (neon)               ── planned
  Value
  Policy / CapabilityProvider
```

It shares its value model and its configuration vocabulary with `plugin-host`, so an
application can move between in-process and out-of-process hosting without a plugin
noticing the difference.

## The surface

| Method | Does |
| --- | --- |
| `load(root)` | discovers and loads every plugin under a root |
| `audit(root)` | reports what plugins request, without running any of their code |
| `list()` / `names()` / `len()` | what is loaded, and who signed it |
| `call(plugin, method, args)` | one method on one plugin |
| `dispatch(method, args)` | the same method on every plugin, collecting one result each |
| `reload(plugin)` | re-reads one plugin from disk |
| `revoke(plugin, capability)` | unbinds a granted capability from a live plugin |

`call_async` and `dispatch_async` exist behind the `async` feature, where a plugin
method may yield.

One plugin failing never affects the others: `load` reports `loaded` and `failures`
side by side, and `dispatch` returns a per-plugin outcome rather than raising.

## Values

Arguments and results cross as a single dynamic type: nil, boolean, integer, float,
string, list, map. It is JSON-shaped so it matches what `plugin-host` puts on the wire,
with one addition — Lua's integer/float split is preserved rather than collapsed.

Lua has one table type doing two jobs, so the boundary has to infer which:

* Keys of exactly `1..=n` become a **list**.
* Anything else becomes a **map**, with keys rendered as strings.
* An empty table becomes an empty **list**. Lua cannot tell an empty sequence from an
  empty mapping, so neither can this.
* A sparse array — `{[1] = "a", [3] = "c"}` — becomes a map with the keys `"1"` and
  `"3"`. Treating it as a list would drop the third element.

Three things are refused rather than silently flattened, because a `nil` the caller
misreads is worse than an error it can see:

* **Functions, threads and userdata.** A plugin returning one was written for an
  in-process Rust host.
* **Non-UTF-8 strings.** Lua strings are byte strings; lossy replacement would corrupt
  a value on its way through a boundary whose job is to carry it faithfully.
* **Tables nested past 64 levels**, which is also what catches a table containing
  itself. Nothing stops a plugin returning `t` where `t.self = t`, and a naive walk of
  that would overflow the stack and take the host process with it.

## Capabilities from foreign code

A Rust host registers capabilities as closures. A Python or Kotlin host has none to
give, so it implements `CapabilityProvider` and the binding hands over an object. Each
capability becomes a Lua function bound in the granted plugin's environment; calling it
reaches back out to whoever provided it — the same mechanism `plugin-host` uses for its
forwarded capabilities, with "out" being a foreign function in this process rather than
a JSON-RPC peer in another one.

The grant [policy approved](capabilities.md) travels with every call, so a provider can
re-check its own bounds instead of trusting the registry to have narrowed correctly.

A provider returning an error surfaces inside Lua as an ordinary runtime error, so a
plugin can `pcall` around it. Refusing is a normal outcome, not a fatal one.

`Policy` is the other half: foreign code decides, per plugin, what is actually granted,
and may narrow the parameters rather than only answering yes or no.

### Do not call back into the registry

A provider runs while the registry that invoked it is locked. Calling back into that
same registry would deadlock — no error, no stack, nothing in a log.

It does not. The attempt returns a `reentrant` error instead. Capture whatever the
provider needs before the call.

Two registries nest fine: different locks, so there is no hazard, and that stays true.

## Threading

One lock guards the whole registry, so calls from several threads serialize rather than
race. A long-running plugin therefore blocks every other caller for its duration —
which is what the instruction and memory limits in [isolation](isolation.md) are for,
and they are on by default.

## Choosing the Lua VM

No binding hard-codes a VM. A binding that did could not be embedded in an application
that already has its own, which would leave two Lua VMs in one address space.

Each binding defaults to Lua 5.4 built from source, so the out-of-the-box install needs
nothing present on the system. Turning the default off and naming a version instead
links against whatever the application already has:

```sh
cargo build -p stanchion-ffi --no-default-features --features lua53,vendored
cargo build -p stanchion-ffi --no-default-features --features luajit   # system LuaJIT
```

Selecting none fails the build in `mlua`, which is the right error to get.

## Python

```sh
mise run test:python           # all of the below
```

```sh
cd bindings/python
uv sync                        # installs the dev group and builds the extension
uv run maturin develop --uv    # rebuild after a Rust change
uv run pytest
```

`pyproject.toml` is the only Python configuration — build backend, metadata, the `dev`
dependency group, pytest, ruff and mypy all live there. `uv.lock` is committed so CI
resolves what a developer resolved.

```python
import stanchion

def kv(call: stanchion.CapabilityCall) -> str:
    # `call.grant` holds what policy approved, not what the manifest asked for.
    return store[call.args[0]]

def policy(request: stanchion.CapabilityRequest) -> stanchion.Decision:
    if request.signer == "unsigned":
        return stanchion.Decision.deny("unsigned plugins get no store access")
    return stanchion.Decision.grant_with({"keys": ["public:*"]})

host = stanchion.Stanchion(capabilities={"kv": kv}, policy=policy)
report = host.load("plugins/")
for failure in report.failures:
    print(failure.plugin, failure.reason)

print(host.call("greeter", "greet", "world"))
print(await host.call_async("greeter", "fetch", "https://example.com"))
```

Failures arrive as exceptions under a `StanchionError` base: `UnknownPluginError`,
`PluginError`, `LuaError`, `ConfigError`, `CapabilityError`, `ReentrantError`. A
provider that raises becomes an ordinary Lua error the plugin can `pcall`; a policy
that raises denies rather than crashing the host.

The GIL is released while Lua runs, so other Python threads and asyncio tasks keep
going. A provider re-acquires it on the same thread, which is why calling back into the
registry raises `ReentrantError` instead of hanging.

Published wheels carry Lua 5.4 built from source and target `abi3` from Python 3.10, so
one wheel per platform covers every supported interpreter. To link an interpreter the
application already embeds:

```sh
uv run maturin develop --no-default-features --features luajit
```

## Kotlin and Swift

Generated with UniFFI from `bindings/uniffi`. The generator is built from that crate
so it always matches the `uniffi` the library was compiled against — a mismatch
produces bindings that compile and then misbehave at the boundary.

```sh
mise run smoke:uniffi               # generates all three, runs the boundary
mise run smoke:kotlin               # compiles and runs the Kotlin on the JVM
```

Both are scripts under `bindings/uniffi/` and can be run directly. `mise.toml` pins
the Kotlin, Java and ktlint versions they expect.

```kotlin
class Store : CapabilityProvider {
    override fun invoke(call: CapabilityCall): Value {
        val key = (call.args.first() as Value.Str).value
        return store[key]?.let { Value.Str(it) } ?: throw ProviderException.Refused("no such key")
    }
}

val host = Stanchion(config, mapOf("kv" to Store()), policy)
host.load("plugins/")
host.call("greeter", "greet", listOf(Value.Str("world")))
host.callAsync("greeter", "fetch", listOf(Value.Str("https://example.com")))
```

`callAsync` and `dispatchAsync` are `suspend` functions in Kotlin and `async` in
Swift. Throwing `ProviderException` from a provider becomes an ordinary Lua error the
plugin can `pcall`; re-entering the registry from one raises `StanchionException.Reentrant`.

Two things are worth knowing about the shape, both forced by Kotlin:

* The sequence and mapping variants are `Value.Seq` and `Value.Table`, not `List` and
  `Map`. A variant named `List` becomes a nested class that shadows
  `kotlin.collections.List` for the rest of the sealed class, so its own field stops
  naming a list and the generated file will not compile.
* `args` has no default, so a call with none passes an empty list.

The Swift bindings generate alongside the Kotlin but have not been run — this is a
Linux checkout with no Swift toolchain. Packaging for both (XCFramework and Swift
Package, AAR with per-ABI libraries) is still to come.

## Configuration

Bindings and `plugin-host` read the same shape, so a policy written for one transport
describes the other unchanged:

```toml
plugins = "plugins/"

[sandbox]
libs = ["string", "table", "math"]
memory_limit = 67108864
instruction_limit = 50000000
shared = false

[capabilities]
allow = ["log"]
callbacks = ["kv"]

[signatures]
required = true
```

Capabilities and policy are fixed before the first plugin loads and cannot be changed
afterwards. A host that could widen a running plugin's reach would have given up the
guarantee the capability system exists to make.

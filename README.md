# stanchion

Lua plugins for Rust applications. A stanchion both bears load and marks a boundary,
which is what this does: typed contracts and dependency wiring on one side, sandboxing,
capabilities and signatures on the other.

## Crates

| Crate                                             | Contains                                                                        |
| ------------------------------------------------- | ------------------------------------------------------------------------------- |
| [`stanchion`](crates/stanchion)                   | facade; re-exports the rest behind features, and ships the `plugin-host` binary |
| [`stanchion-core`](crates/stanchion-core)         | `#[lua_class]` contract: `LuaClass`, `LuaObject`, `BoxFuture`                   |
| [`stanchion-macros`](crates/stanchion-macros)     | the attribute macro                                                             |
| [`stanchion-registry`](crates/stanchion-registry) | manifests, sandboxing, capabilities, signatures, reload                         |
| [`stanchion-rocks`](crates/stanchion-rocks)       | LuaRocks tree queries and version constraints                                   |
| [`stanchion-sigstore`](crates/stanchion-sigstore) | sigstore verification — quarantines a large dependency graph                    |
| [`stanchion-dist`](crates/stanchion-dist)         | packaging, the index protocol, OCI transport, pinned installs                   |
| [`stanchion-remote`](crates/stanchion-remote)     | out-of-process hosting: JSON-RPC protocol, client, host                         |
| [`stanchion-ffi`](crates/stanchion-ffi)           | the seam the language bindings sit on: dynamic calls, values, capabilities      |

Depend on `stanchion` and pick features; generated code refers to `::stanchion`, so use
the macro through the facade.

## Binding a Lua class to a Rust trait

```rust
use stanchion::{load_class, lua_class, mlua::{Lua, Result}};

#[lua_class]
pub trait Greeter {
    /// `Greeter.new(greeting)` — no receiver, so it lands on `GreeterClass`
    fn new(greeting: String) -> Result<Self>;

    /// `obj:greet(who)`
    fn greet(&self, who: String) -> Result<String>;

    /// `obj.greeting`
    #[lua(field)]
    fn greeting(&self) -> Result<String>;

    /// called only if the Lua class defines it
    #[lua(optional)]
    fn on_unload(&self) -> Result<Option<()>>;

    /// `obj:fetch(url)` as a coroutine (requires the `async` feature)
    async fn fetch(&self, url: String) -> Result<String>;
}
```

```lua
local Greeter = {}
Greeter.__index = Greeter

function Greeter.new(greeting)
  return setmetatable({ greeting = greeting }, Greeter)
end

function Greeter:greet(who)
  return self.greeting .. ", " .. who
end

return Greeter
```

```rust
let class: GreeterClass = load_class(&lua, source, "greeter.lua")?;
let greeter = class.new("hello".to_string())?;   // -> GreeterHandle
assert_eq!(greeter.greet("world".to_string())?, "hello, world");
```

## Documentation

| Guide                                     | Covers                                                                    |
| ----------------------------------------- | ------------------------------------------------------------------------- |
| [Typed class bindings](docs/lua-class.md) | what `#[lua_class]` generates, its attributes, what it validates          |
| [Plugin registry](docs/registry.md)       | manifests, discovery, dispatch, reload, failure isolation                 |
| [Isolation](docs/isolation.md)            | shared vs per-plugin states, sandbox policy, resource limits              |
| [Capabilities](docs/capabilities.md)      | declared authority, policy, narrowing, audit, revocation                  |
| [Signatures](docs/signatures.md)          | directory digests, sigstore, provenance-tiered capabilities               |
| [Distribution](docs/distribution.md)      | OCI packages, an index anyone can run, lockfile pinning, upgrade review   |
| [Dependency chains](docs/dependencies.md) | published exports, semver requirements, reload propagation                |
| [LuaRocks](docs/luarocks.md)              | declaring external Lua libraries, C-module hazards                        |
| [Out-of-process hosting](docs/remote.md)  | the `plugin-host` binary, JSON-RPC protocol, callbacks, crash containment |
| [Language bindings](docs/bindings.md)     | Python, Kotlin, Swift, Ruby and JavaScript over the dynamic surface       |
| [Testing](docs/testing.md)                | running the suite under nextest, profiles, doctests, UI tests             |

## Examples

Runnable, and built by `cargo build --examples`, so they cannot rot into
documentation that no longer compiles:

| Example                                                   | Shows                                                                                                                                |
| --------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------ |
| [`greeter`](crates/stanchion/examples/greeter.rs)         | the typed contract alone — no registry; Lua and native implementations behind one `dyn` trait                                        |
| [`event_bus`](crates/stanchion/examples/event_bus.rs)     | a plugin directory: dependency ordering, published `exports`, failure isolation at load and dispatch                                 |
| [`untrusted`](crates/stanchion/examples/untrusted.rs)     | running code you did not write: sandbox, memory and instruction limits, a policy that narrows a capability, revocation, static audit |
| [`remote_host`](crates/stanchion/examples/remote_host.rs) | plugins in a child process, calling back into the application — and surviving one that kills its host                                |

```sh
cargo run -p stanchion --features lua54,vendored,registry --example untrusted
```

```text
greedy   os=nil io=nil kv=function -> sandboxed/greedy/some-key
looper   stopped: runtime error: plugin exceeded its instruction limit of 200000
```

The plugin sources they load are beside them, under
[`crates/stanchion/examples/`](crates/stanchion/examples).

## Features

| Feature                                        | Effect                                                                                                                             |
| ---------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| `async`                                        | enables `mlua/async`; `async fn` becomes `fn(..) -> BoxFuture<'_, Result<T>>`                                                      |
| `send`                                         | enables `mlua/send`; traits gain `MaybeSend + MaybeSync` supertraits (so `dyn Trait: Send + Sync`) and `BoxFuture` requires `Send` |
| `registry`                                     | adds `Registry<C>`: manifest discovery, semver dependency chains, config, reload. Pulls in `toml`, `semver` and `mlua/serde`       |
| `luarocks`                                     | adds `[rocks]` declarations, tree verification and `install_rocks`. Shells out to `luarocks`; no new crates                        |
| `signatures`                                   | adds `DirectoryDigest`, the `PluginVerifier` seam, and provenance in capability policy. Pulls in `sha2`                            |
| `remote`                                       | adds the `plugin-host` binary and `RemoteRegistry` client, speaking JSON-RPC 2.0. Pulls in `serde_json` and `jsonrpsee-types`      |
| `sigstore-verify`                              | adds `SigstoreVerifier` for keyless verification. Pulls in `sigstore` and ~246 transitive crates                                   |
| `full`                                         | everything above except `sigstore-verify`; what the test suite runs with                                                           |
| `lua54`, `lua53`, `luajit`, `luau`, `vendored` | forwarded to `mlua`                                                                                                                |

Async methods return a boxed future rather than using `async fn` in traits, which keeps
them dyn-compatible.

## Limitations

- Generic traits are rejected — a Lua class has no type parameters.
- A registry holds one class type. Mixing native Rust implementations into the same
  registry would need it to store `Box<dyn Trait>`, which cannot be reached generically
  from `C::Instance` on stable (the unsizing coercion is not expressible as a bound).
- Per-plugin isolation and `[dependencies]` are mutually exclusive: Lua values cannot
  cross states, so a plugin chain needs shared isolation.
- In-process isolation bounds CPU and memory, but cannot survive a crash inside the
  interpreter; use the `remote` feature when that matters.
- Capabilities bound what a plugin can reach, not what it does with what it got, and
  they are only as strong as the providers that enforce their grants.
- A revocation list is consulted at load, so a plugin already running when an entry is
  added keeps running until it is reloaded.
- `SigstoreVerifier` reports the identity your policy enforced, because sigstore's
  verification API answers conformance rather than returning the certificate subject.
  different implementation.

## Panic policy

Declared once in `[workspace.lints]` and inherited by every crate, so it cannot drift
as crates are added:

```toml
[workspace.lints.clippy]
unwrap_used = "deny"
expect_used = "deny"
panic = "deny"
unreachable = "deny"
indexing_slicing = "deny"
arithmetic_side_effects = "deny"
```

Every fallible path returns an error instead: `mlua::Error` from generated method
bodies, `syn::Error` with a span from the macro, and `RegistryError` / `LoadFailure`
from the registry. Tests propagate too, so a broken assumption reports its own message
rather than an unwrap location.

## Testing

```sh
cargo nextest run --workspace --features lua54,vendored,full
```

See [docs/testing.md](docs/testing.md) for profiles, doctests and the UI tests.

## Licence

Dual-licensed under either [MIT](LICENSE-MIT) or
[Apache-2.0](LICENSE-APACHE), at your option.

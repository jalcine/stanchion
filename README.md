# stanchion

Lua plugins for Rust applications. A stanchion both bears load and marks a boundary,
which is what this does: typed contracts and dependency wiring on one side, sandboxing,
capabilities and signatures on the other.

## Crates

| Crate | Contains |
| --- | --- |
| [`stanchion`](crates/stanchion) | facade; re-exports the rest behind features |
| [`stanchion-core`](crates/stanchion-core) | `#[lua_class]` contract: `LuaClass`, `LuaObject`, `BoxFuture` |
| [`stanchion-macros`](crates/stanchion-macros) | the attribute macro |
| [`stanchion-registry`](crates/stanchion-registry) | manifests, sandboxing, capabilities, signatures, reload |
| [`stanchion-rocks`](crates/stanchion-rocks) | LuaRocks tree queries and version constraints |
| [`stanchion-sigstore`](crates/stanchion-sigstore) | sigstore verification — quarantines a large dependency graph |
| [`stanchion-remote`](crates/stanchion-remote) | out-of-process hosting and the `plugin-host` binary |

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

## What the macro generates

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

## Plugin registry

The `registry` feature adds `Registry<C>`, which loads a directory of plugins into
either one shared Lua state or one state per plugin.

```
plugins/
  greeter/
    plugin.toml
    init.lua
  formatter/
    plugin.toml
    init.lua
```

```toml
# plugins/greeter/plugin.toml
name = "greeter"
version = "1.2.0"     # semver; absent means 0.0.0, which satisfies only "*"
entry = "init.lua"    # optional, defaults to init.lua

[dependencies]
formatter = "^1.0"
logger = { version = "^2.0", optional = true }

[config]
greeting = "hello"
```

The constructor (`new` by default, `Registry::with_constructor` to change it) receives
the manifest's `[config]` table and a `deps` table holding what this plugin is wired to:

```lua
function Greeter.new(config, deps)
  return setmetatable({
    greeting = config.greeting,
    fmt = deps.formatter,          -- nil if declared optional and absent
  }, Greeter)
end
```

Lua ignores extra arguments, so a plugin that only wants `config` can keep taking one.

```rust
let mut registry: Registry<GreeterClass> = Registry::new(Lua::new());
let report = registry.load_dir("plugins/")?;

for outcome in registry.dispatch(|p| p.greet("world".to_string())) {
    match outcome.result {
        Ok(text) => println!("{}: {text}", outcome.name),
        Err(err) => eprintln!("{} failed: {err}", outcome.name),
    }
}
```

### Isolation

Two modes, chosen at construction.

```rust
// Shared: every plugin runs in the state you hand over.
let registry = Registry::new(Lua::new());

// Per-plugin: every plugin gets its own state under a policy.
let registry = Registry::isolated(
    Lua::new(),
    Sandbox::restricted()
        .memory_limit(16 * 1024 * 1024)
        .instruction_limit(5_000_000),
);
```

| | Shared | Per-plugin |
| --- | --- | --- |
| Memory / instruction limits | impossible | enforced by the VM |
| Standard library selection | one policy, set by you | per registry, via `Sandbox` |
| `[dependencies]` exports | injected | **unavailable** — values cannot cross states |
| Blast radius of a bad plugin | shared globals and allocator | contained to its own state |
| Cost | one state | one state per plugin |

Under **shared** isolation each chunk is evaluated with its own environment table whose
`__index` is the real globals: a plugin **reads** globals normally but its **writes**
stay local, so one plugin cannot redefine `string.format` for the others. That is
namespace hygiene, not a security boundary — `rawset(_G, ...)` still reaches the shared
state, and nothing bounds CPU or memory.

Under **per-plugin** isolation the boundary is real. Memory and instruction limits are
properties of a `Lua`, which is exactly why they cannot be applied inside a shared
state. The price is that Lua values cannot cross states, so a `[dependencies]` entry
that would inject exports fails with `CrossStateDependency`.

Each plugin's directory is prepended to its state's `package.path`, so a plugin can
`require` its own files without knowing where it was installed. That `require` is
per-plugin: submodules load into the **same environment** as the plugin's own chunk, so
they see its granted capabilities, and each plugin gets its own module cache rather
than sharing `package.loaded`.

#### Sandbox policy

`Sandbox::restricted()` is the default: `string`, `table`, `math`, `coroutine` and
`package` — no `io`, `os` or `debug` — with `dofile`, `loadfile` and `package.loadlib`
removed on top. `package` is loaded so `require` works for rocks and plugin-local
modules; `loadlib` is removed because it would load any shared object on disk.

`Sandbox::permissive()` adds `io` and `os` and drops the deny list. Use it for
first-party plugins, not for code you did not write.

Note that mlua's `StdLib::ALL_SAFE` means *memory*-safe, not sandboxed — it still
includes `io` and `os`. Neither preset is built on it.

```rust
Sandbox::restricted()
    .libs(StdLib::STRING | StdLib::TABLE)   // exactly these
    .deny(["os.execute", "os.exit"])        // replace the deny list
    .memory_limit(16 * 1024 * 1024)         // bytes, VM-enforced
    .instruction_limit(5_000_000)           // per call, not per lifetime
```

The instruction limit resets at every call boundary — each `dispatch` and each
constructor gets the full allowance — so a plugin is bounded per call rather than
slowly starving over its lifetime. It rides on Lua's debug hook, so under `luau`
(which has no instruction counter) the limit counts interrupt callbacks instead.

### Capabilities

`with_setup` is where the host declares **everything** a plugin can reach. Capabilities
are gated; ambient values are not, and both are reported by `audit`.

```rust
let registry = Registry::isolated(Lua::new(), Sandbox::restricted())
    .with_setup(|host| {
        host.capability("network", |lua, grant| {
            // The allowlist is baked into the value, so the plugin cannot widen it.
            let allowed: Vec<String> = grant.get_or_default("hosts");
            Ok(Value::Function(lua.create_function(move |_, url: String| {
                if allowed.contains(&url) { fetch(&url) } else { Err(denied()) }
            })?))
        });
        host.ambient("HOST_VERSION", |lua| lua.globals().set("HOST_VERSION", "1.0"));
        Ok(())
    })
    .with_policy(Rules::deny_all().allow_with("network", narrow_to_one_host));
```

A plugin requests what it needs in its manifest:

```toml
[capabilities.network]
hosts = ["api.example.com"]

[capabilities.metrics]
optional = true          # reserved key; stripped before the provider sees params
```

Granted names are bound in that plugin's environment, so the class contract is
untouched — constructors stay `new(config, deps)`:

```lua
function Weather:fetch()
  return network(self.host)   -- nil if never granted
end
```

**Deny by default.** Registering a provider says the host *can* offer something;
the policy decides who *gets* it. Without a policy every capability is refused, even
one with a provider.

**The policy can narrow, not just refuse.** A host that can only answer yes or no to
`hosts = ["*"]` is a rubber stamp. `Decision::GrantWith` replaces the requested
parameters, and the provider closes over the approved ones — so there is no parameter
left for the plugin to tamper with at call time.

| Outcome | Required capability | Optional capability |
| --- | --- | --- |
| Policy grants | bound in the environment | bound in the environment |
| Policy denies | `CapabilityDenied`, plugin fails | left unbound, plugin loads |
| No provider offered | `UnknownCapability`, plugin fails | left unbound, plugin loads |

#### Audit, introspection, revocation

```rust
let audit = registry.audit("plugins/")?;      // executes nothing
for request in audit.unsatisfiable() { /* asks for what the host cannot offer */ }
audit.ambient;                                 // reaches every plugin regardless

plugin.granted_capabilities();                 // what policy actually allowed
registry.revoke("weather", "network")?;        // unbind on a live plugin
```

`audit` reads manifests only, so you can review what a plugin wants **before running
any of its code** — the precondition for accepting plugins you did not write. Revoking
sets the name to `nil` in the plugin's environment; code that already captured the
value in a local keeps it, so revocation defangs a plugin rather than rewinding it.

#### What this does not buy

- **A sloppy provider defeats it.** If a provider ignores its `Grant` and reads any
  path, the declaration is a comment. Enforcement lives in the provider.
- **It bounds reach, not use.** A plugin granted one host can still hammer that host.
- **Manifests are self-asserted.** A modest declaration only means something once
  signatures make the manifest attributable.

### Signatures

A signature binds **an identity** to **the exact bytes that will run**, checked against
a trust root the host controls before any Lua executes.

#### What is signed

Every file in the plugin directory, not just `plugin.toml`. Signing the manifest alone
would be worse than useless: swap `init.lua`, leave the manifest, and an audit reports
capabilities the code does not match. `DirectoryDigest` hashes each file and folds them
in sorted order:

```text
for each file, sorted by path (excluding the signature artifact):
    relative path ‖ 0x00 ‖ sha256(contents) ‖ 0x00
```

Those bytes — `DirectoryDigest::preimage` — are the signed artifact, so plain `cosign`
can produce a bundle without any bespoke tool. The per-file hashes are kept, and the
loader re-checks each file as it reads it, so what runs is what was verified even if
the directory changed in between.

#### Sigstore

The `sigstore-verify` feature provides keyless verification: the author signs with an
OIDC identity, Fulcio issues a short-lived certificate, and the host checks the bundle
against sigstore's trust root plus an identity policy of its choosing. Authors hold no
long-term key, and the host trusts an *identity* rather than a key fingerprint.

```rust
use sigstore::bundle::verify::policy::GitHubWorkflowRepository;

let registry = Registry::isolated(Lua::new(), Sandbox::restricted())
    .with_verifier(SigstoreVerifier::production(
        "repo:acme/plugins",
        GitHubWorkflowRepository::new("acme/plugins"),
    )?)
    .require_signatures(true);
```

Verification is offline by default — loading plugins should not depend on reaching a
log server; the bundle's inclusion proof is checked either way.

Two costs worth knowing. `sigstore-verify` pulls roughly **246 transitive
dependencies**, including TUF, HTTP and TLS stacks, which is why it is optional. And
sigstore-rs describes itself as experimental with an API that can change — which is why
verification sits behind the `PluginVerifier` trait rather than being wired in
directly, so you can supply your own PKI instead.

#### Unsigned plugins

`require_signatures(false)`, the default, loads an unsigned plugin as
`Signer::Unsigned`. That makes signing a gradient rather than a cliff — existing
plugins keep working, and policy decides what they may reach.

#### Provenance tiers capabilities

This is where signing stops being a checkbox. `CapabilityRequest` carries the verified
signer, so the policy can grant by who signed:

```rust
Rules::deny_all().allow_with("network", |request| match request.signer().identity() {
    Some(id) if id.starts_with("repo:acme/") => Decision::Grant,
    _ => Decision::deny("network requires a first-party signature"),
})
```

Pair that with `optional = true` and an unsigned plugin degrades gracefully instead of
failing: it loads, just without the capability.

`audit` reports each plugin's signer alongside its requests, still without running any
plugin code.

#### What signatures do not buy

- **Origin and integrity, not safety.** A verified plugin from a trusted author can
  still be hostile. A signature says whom to hold responsible.
- **Revocation is separate.** A withdrawn plugin's signature stays valid; that needs a
  denylist the host refreshes.

### Dependency chains

`[dependencies]` is a material wiring, not just load order. A plugin publishes what it
wants dependents to see by declaring `exports` — a table, or a method returning one:

```lua
function Formatter:exports()
  local style = self.style
  return {
    decorate = function(text) return style .. text end,
  }
end
```

Dependents receive **only** that table. Instance methods and fields the plugin did not
publish stay private, so a dependency's internals never become de-facto API. Depending
on a plugin that publishes nothing fails with `MissingExports`.

Versions are semver, matched with the [`semver`](https://docs.rs/semver) crate:
requirements and versions deserialize straight from the manifest, so a malformed one is
a manifest error rather than a runtime surprise. A dependency present at an unsatisfying
version fails with `IncompatibleDependency` before any Lua runs.

Only direct dependencies are injected — a plugin that declares `b` does not
automatically receive `b`'s own dependencies.

### Reload

`registry.reload("greeter")` re-reads the manifest and chunk and swaps in a fresh
instance. A handle to the plugin's own instance taken before the reload keeps talking to
the old object until it is dropped, so in-flight calls do not break.

Dependents do not receive an exports table directly; they receive a stable **proxy**
whose metatable forwards reads and writes to the current one. Reloading a provider
repoints that metatable, so every dependent sees the new surface without being rebuilt
and without holding a stale capture:

```rust
let beta = registry.get("beta").unwrap().instance().clone();
assert_eq!(beta.greet("world".to_string())?, "[hello] world");

registry.reload("alpha")?;   // beta is never rebuilt

assert_eq!(beta.greet("world".to_string())?, "[howdy] world");
```

Forwarding `__newindex` as well as `__index` is what keeps this honest: a method doing
`self.count = self.count + 1` through the proxy writes to the real table instead of
shadowing the field on the proxy and silently forking state.

Two caveats: `rawget`/`rawset` bypass the proxy, and identity comparisons see the proxy
rather than the exports table. A reload that withdraws a previously published `exports`
fails and leaves the old instance in place.

### LuaRocks

With the `luarocks` feature, a plugin declares the external libraries it needs:

```toml
[rocks]
dkjson = "2.11"
lpeg = ">= 1.0, < 2.0"
inspect = "~> 3.1"
```

```rust
let registry = Registry::new(Lua::new())
    .with_rocks(RocksConfig::new("/var/lib/myapp/rocks"));
```

The registry puts that tree on the shared state's `package.path`, then verifies each
plugin's declarations against what the tree holds. A plugin declaring `[rocks]` without
a configured tree fails to load rather than silently resolving `require` from whatever
happens to be on the machine.

**Loading never installs.** Provisioning is an explicit host call:

```rust
let report = registry.install_rocks("plugins/")?;   // before load_dir
```

so `load_dir` never touches the network, takes unbounded time, or mutates the machine.
Note that `luarocks install` takes a version rather than a constraint expression, so a
range requirement installs the newest available version and is checked afterwards.

Rock versions are **not semver** — `2.11-1` carries a rockspec revision, and constraints
use LuaRocks' own operators (`~>`, comma-separated ranges). They are compared
component-wise by this crate rather than by the `semver` crate that handles
plugin-to-plugin dependencies. A bare `2.11` accepts an installed `2.11-1`; a
requirement that spells out a revision does not.

#### C modules

`package.cpath` is left alone unless you opt in:

```rust
RocksConfig::new(tree).load_c_modules(true)
```

A C rock is a shared object linked against some `liblua`. With `vendored`, mlua links
Lua statically into the host binary, so loading one can pull a **second Lua runtime**
into the process — a crash or silent corruption, not a clean error. Pure-Lua rocks are
unaffected. Only enable this when mlua links against the same external Lua the rocks
were built against.

### Failure isolation

Only an unreadable plugin root is fatal. Everything else lands in
`LoadReport::failures` while the remaining plugins still load:

| `FailureReason` | Cause |
| --- | --- |
| `Manifest` | `plugin.toml` is not valid TOML, or a name collides |
| `Io` | the manifest or entry file could not be read |
| `Lua` | the chunk failed, returned an invalid class, or the constructor errored |
| `MissingDependency` | a required dependency is not in the plugin root |
| `IncompatibleDependency` | a dependency is present, but its version fails the requirement |
| `MissingExports` | a dependency publishes no `exports` table |
| `MissingRock` | a declared rock is not installed in the tree |
| `IncompatibleRock` | a declared rock is installed at an unsatisfying version |
| `Rocks` | rocks were declared with no tree configured, or the feature is off |
| `CrossStateDependency` | a dependency's exports cannot reach a per-plugin state |
| `CapabilityDenied` | policy refused a capability the plugin requires |
| `UnknownCapability` | the plugin requested something the host does not offer |
| `Unsigned` | no signature, and the registry requires one |
| `SignatureInvalid` | a signature was present but did not verify |
| `UntrustedSigner` | cryptographically sound, but the signer is not trusted |
| `DigestMismatch` | a file changed between verification and loading |
| `DependencyFailed` | a dependency failed, so this plugin was skipped |
| `DependencyCycle` | this plugin is part of a cycle |

Dispatch is isolated the same way: `dispatch` returns one `Outcome` per plugin, so a
plugin that errors does not stop the others.

### Async dispatch

With `async` + `registry`, `dispatch_async` awaits each plugin in turn. Calls are
sequential by design: they all reach the same Lua state, so running them concurrently
would only contend on it.

## Out-of-process hosting

Everything above bounds what a plugin can *reach*. None of it bounds what a plugin can
do to the process it runs in: a sandbox cannot stop a segfault in a C rock, and an
instruction limit cannot rescue a state whose allocator already failed. The `remote`
feature moves plugins into a child process, where those failures are survivable.

```text
plugin-host --config host.toml --plugins plugins/
```

```rust
let mut remote = RemoteRegistry::launch(
    RemoteOptions::new("./plugin-host").config("host.toml").plugins("plugins/"),
)?;

let greeting: String = remote.call("greeter", "greet", [json!("world")])?;
for outcome in remote.dispatch("on_event", [json!({"kind": "tick"})])? {
    match (outcome.value, outcome.error) { /* per-plugin, as in-process */ }
}
```

### Transport and protocol

Length-prefixed JSON over **stdio** — a little-endian `u32` byte count then that many
bytes of JSON. No ports, no socket files, and stderr stays free for logs, which is
where the host's built-in `log` capability writes. JSON rather than a compact binary
encoding because the channel is a debugging surface as much as a transport.

Calls are **dynamically typed**, and that is forced rather than chosen: a host binary
is compiled before anyone writes a plugin, so it cannot know a `#[lua_class]` trait.
Plugins load as `DynClass` — any table with a constructor — and methods resolve by name
at call time. Arguments and results cross as JSON, converted at the Lua boundary by
`mlua`'s serde support. In-process hosting keeps the typed contract; use it when you
can.

### What a crash looks like

| In-process | Out-of-process |
| --- | --- |
| `while true do end` | instruction limit, either way |
| allocation storm | memory limit, either way |
| `os.exit` / segfault in a C rock | **your application dies** / `RemoteError::HostGone` |
| interpreter panic | unwinds into your stack / child dies, you keep serving |

`RemoteError::HostGone { status }` is the interesting one: the client distinguishes a
dead child from an ordinary transport error by reaping it, so a plugin that kills its
process becomes one error value rather than an outage. Dropping a `RemoteRegistry`
closes the host's stdin and kills anything that ignores it, so a dropped client never
leaks a process.

### Host configuration

Policy comes from a file the operator controls, never from plugin manifests:

```toml
plugins = "plugins/"

[sandbox]
libs = ["string", "table", "math", "coroutine", "package"]
deny = ["dofile", "loadfile", "package.loadlib"]
memory_limit = 67108864
instruction_limit = 50000000
shared = false            # per-plugin states by default

[capabilities]
allow = ["log"]

[signatures]
required = false
```

`shared` defaults to `false`: a separate process should not stop isolating at the
process boundary. An unknown standard-library name is a configuration error rather than
a silent omission.

### Current limitation

The host offers exactly one capability, `log`, because a capability provider is a Rust
closure and the generic binary has none of yours. Plugins in the child can therefore
compute, but cannot call back into your application — which suits transforms, rules and
policies, and does not suit plugins that need host services. Bidirectional RPC
(a capability that forwards to the parent and awaits a reply, the way Neovim's remote
plugins work) is the next step; the protocol already carries correlation ids for it.

## Method attributes

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

## Validation

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

## Features

| Feature | Effect |
| --- | --- |
| `async` | enables `mlua/async`; `async fn` becomes `fn(..) -> BoxFuture<'_, Result<T>>` |
| `send` | enables `mlua/send`; traits gain `MaybeSend + MaybeSync` supertraits (so `dyn Trait: Send + Sync`) and `BoxFuture` requires `Send` |
| `registry` | adds `Registry<C>`: manifest discovery, semver dependency chains, config, reload. Pulls in `toml`, `semver` and `mlua/serde` |
| `luarocks` | adds `[rocks]` declarations, tree verification and `install_rocks`. Shells out to `luarocks`; no new crates |
| `signatures` | adds `DirectoryDigest`, the `PluginVerifier` seam, and provenance in capability policy. Pulls in `sha2` |
| `remote` | adds the `plugin-host` binary and `RemoteRegistry` client. Pulls in `serde_json` |
| `sigstore-verify` | adds `SigstoreVerifier` for keyless verification. Pulls in `sigstore` and ~246 transitive crates |
| `lua54`, `lua53`, `luajit`, `luau`, `vendored` | forwarded to `mlua` |

Async methods return a boxed future rather than using `async fn` in traits, which keeps
them dyn-compatible.

## Limitations

- Methods must return `mlua::Result<..>`; other error types are not unwrapped.
- Generic traits are rejected — a Lua class has no type parameters.
- Handles wrap tables only; userdata-backed classes are not yet supported, though
  `ObjectLike` would allow it.
- A registry holds one class type. Mixing native Rust implementations into the same
  registry would need it to store `Box<dyn Trait>`, which cannot be reached generically
  from `C::Instance` on stable (the unsizing coercion is not expressible as a bound).
- Per-plugin isolation and `[dependencies]` are mutually exclusive: Lua values cannot
  cross states, so a plugin chain needs shared isolation.
- In-process isolation bounds CPU and memory, but cannot survive a crash inside the
  interpreter; use the `remote` feature when that matters.
- The out-of-process host cannot call back into your application yet, so remote plugins
  are limited to computation plus `log`.
- Capabilities bound what a plugin can reach, not what it does with what it got, and
  they are only as strong as the providers that enforce their grants.
- Revocation is unimplemented: a withdrawn plugin's signature remains valid, so a host
  needs its own denylist.
- `SigstoreVerifier` reports the identity your policy enforced, because sigstore's
  verification API answers conformance rather than returning the certificate subject.

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

## Licence

Dual-licensed under either [MIT](LICENSE-MIT) or
[Apache-2.0](LICENSE-APACHE), at your option.

## Testing

Per-crate unit tests plus integration tests in the facade:

```sh
cargo test --features lua54,vendored
cargo test --features lua54,vendored,registry
cargo test --features lua54,vendored,async,send
cargo test --features lua54,vendored,async,send,registry
cargo test --features lua54,vendored,async,send,registry,luarocks
cargo test --features lua54,vendored,async,send,registry,signatures
cargo test --features lua54,vendored,remote
```

The `luarocks` tests build a rock offline with `luarocks make` from a local rockspec, so
they need no network. They skip themselves if `luarocks` is not on `PATH`.

`tests/ui/` holds `trybuild` compile-fail cases pinning the macro's diagnostics. They
only assert on `error:` lines the macro itself emits, so they are not sensitive to rustc
version. After deliberately changing a message, refresh the expectations with:

```sh
TRYBUILD=overwrite cargo test --features lua54,vendored --test ui
```

A mismatch writes the actual output to `wip/` for comparison.

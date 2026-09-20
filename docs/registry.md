# Plugin registry

Discovering, loading, calling and reloading a directory of plugins.

See also [isolation](isolation.md), [capabilities](capabilities.md),
[dependency chains](dependencies.md), [signatures](signatures.md),
[LuaRocks](luarocks.md) and [out-of-process hosting](remote.md).

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


## Reload

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


## Failure isolation

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
| `Revoked` | the build or its signer is on the host's [revocation list](signatures.md#revocation) |
| `DependencyFailed` | a dependency failed, so this plugin was skipped |
| `DependencyCycle` | this plugin is part of a cycle |

Dispatch is isolated the same way: `dispatch` returns one `Outcome` per plugin, so a
plugin that errors does not stop the others.


## Async dispatch

With `async` + `registry`, `dispatch_async` awaits each plugin in turn. Calls are
sequential by design: they all reach the same Lua state, so running them concurrently
would only contend on it.

---

[← Documentation index](../README.md#documentation)

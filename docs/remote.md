# Out-of-process hosting

Running plugins in a child process so a crash inside the interpreter is
survivable.

# Out-of-process hosting

[Isolation](isolation.md) and [capabilities](capabilities.md) bound what a plugin can
*reach*. Neither bounds what a plugin can do to the process it runs in: a sandbox cannot stop a segfault in a C rock, and an
instruction limit cannot rescue a state whose allocator already failed. The `remote`
feature moves plugins into a child process, where those failures are survivable.

```sh
cargo install stanchion --features lua54,vendored,remote   # provides `plugin-host`
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

## Transport and protocol

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

## What a crash looks like

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

## Host configuration

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

## Current limitation

The host offers exactly one capability, `log`, because a capability provider is a Rust
closure and the generic binary has none of yours. Plugins in the child can therefore
compute, but cannot call back into your application — which suits transforms, rules and
policies, and does not suit plugins that need host services. Bidirectional RPC
(a capability that forwards to the parent and awaits a reply, the way Neovim's remote
plugins work) is the next step; the protocol already carries correlation ids for it.

---

[← Documentation index](../README.md#documentation)

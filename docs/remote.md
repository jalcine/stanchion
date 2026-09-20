# Out-of-process hosting

[Isolation](isolation.md) and [capabilities](capabilities.md) bound what a plugin can
*reach*. Neither bounds what a plugin can do to the process it runs in: a sandbox
cannot stop a segfault in a C rock, and an instruction limit cannot rescue a state
whose allocator already failed. The `remote` feature moves plugins into a child
process, where those failures are survivable.

```sh
cargo install stanchion --features lua54,vendored,remote   # provides `plugin-host`
plugin-host --config host.toml --plugins plugins/
```

```rust
use serde_json::json;
use stanchion::remote::{RemoteOptions, RemoteRegistry};

let mut remote = RemoteRegistry::launch(
    RemoteOptions::new("./plugin-host").config("host.toml").plugins("plugins/"),
)?;

let greeting: String = remote.call("greeter", "greet", [json!("world")])?;

for outcome in remote.dispatch("on_event", [json!("tick")])? {
    match (outcome.value, outcome.error) {
        (Some(value), _) => println!("{}: {value}", outcome.plugin),
        // Per-plugin, exactly as in-process dispatch behaves.
        (_, Some(error)) => eprintln!("{} failed: {error}", outcome.plugin),
        _ => {}
    }
}
```

A [runnable example](../crates/stanchion/examples/remote_host.rs) covers the whole
path, including a plugin that kills its own process.

## Transport and protocol

**JSON-RPC 2.0 over stdio**, one message per line. Not a bespoke encoding: a host can
be written in any language that can read newline-delimited JSON on a pipe. No ports, no
socket files, and stderr stays free for logs, which is where the host's built-in `log`
capability writes.

Methods are namespaced:

| Method | Does |
| --- | --- |
| `plugins/load` | discover and load every plugin under a root |
| `plugins/list` | report the loaded plugins |
| `plugins/audit` | report what plugins request, running none of their code |
| `plugins/call` | call one method on one plugin |
| `plugins/dispatch` | call the same method on every plugin |
| `plugins/reload` | re-read one plugin from disk |
| `plugins/revoke` | unbind a capability from a live plugin |
| `host/info` | describe the host |
| `host/shutdown` | finish serving and exit |
| `capability/<name>` | **host to application**: a plugin calling a capability |

[`jsonrpsee-types`](https://docs.rs/jsonrpsee-types) supplies the parts where
conformance matters — request ids, the `"2.0"` marker, the standard error codes. The
framing lives in this crate because jsonrpsee's own transports are HTTP and WebSocket
only. Envelopes are owned rather than borrowed, since a message read off a pipe
outlives the buffer it arrived in.

Calls are **dynamically typed**, and that is forced rather than chosen: a host binary
is compiled before anyone writes a plugin, so it cannot know a `#[lua_class]` trait.
Plugins load as `DynClass` — any table with a constructor — and methods resolve by name
at call time. Arguments and results cross as JSON, converted at the Lua boundary by
`mlua`'s serde support. In-process hosting keeps the typed contract; use it when you
can.

## Calling back into your application

The channel is bidirectional. A capability listed under `callbacks` becomes a Lua
function that forwards to the process that launched the host:

```toml
[capabilities]
allow = ["log"]        # answered by the host itself
callbacks = ["kv"]     # forwarded to your application
```

```rust
use stanchion::remote::CallbackCall;

let mut remote = RemoteRegistry::launch(options)?.on_callback(|call: &CallbackCall| {
    match call.capability.as_str() {
        "kv" => Ok(json!(lookup(&call.args))),
        other => Err(format!("`{other}` is not offered")),
    }
});
```

Three properties worth knowing:

- **The approved grant travels with every call.** `call.grant` carries the parameters
  the host's policy approved, so your application re-checks rather than trusting the
  host to have narrowed correctly.
- **Declaration still governs.** A plugin that does not request `kv` in its manifest
  never gets it, even though the host offers it — capabilities stay declared-only
  across the process line.
- **Nothing here is fatal.** An application returning `Err` surfaces a Lua error in the
  plugin; an application with no handler at all fails the call with a clear message.
  Both leave the host and the application running.

Requests that arrive while a callback is in flight are queued rather than dropped, so
your application can keep sending while a plugin is mid-call.

## What a crash looks like

| | In-process | Out-of-process |
| --- | --- | --- |
| `while true do end` | instruction limit | instruction limit |
| allocation storm | memory limit | memory limit |
| `os.exit`, or a segfault in a C rock | **your application dies** | `RemoteError::HostGone` |
| interpreter panic | unwinds into your stack | child dies, you keep serving |

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
memory_limit = 67108864        # default 64 MiB
instruction_limit = 50000000   # default 50M, per call
shared = false                 # default: a state per plugin

[capabilities]
allow = ["log"]
callbacks = []

[signatures]
required = false
```

Every key is optional; the defaults above are what the host uses with no config file at
all. `shared` defaults to `false` because a separate process should not stop isolating
at the process boundary. An unknown standard-library name is a configuration error
rather than a silent omission.

## Client surface

`RemoteRegistry` mirrors the in-process registry, minus the typing:

| Method | Returns |
| --- | --- |
| `load(root)` | `LoadResult { loaded, failures }` |
| `list()` | `Vec<PluginInfo>` — name, version, granted capabilities, signer |
| `audit(root)` | `Vec<AuditEntry>` — requests and signer, nothing executed |
| `call(plugin, method, args)` | any `DeserializeOwned` |
| `dispatch(method, args)` | `Vec<Outcome>` — one per plugin |
| `reload(plugin)`, `revoke(plugin, capability)` | `()` |
| `info()` | `HostInfo { version, isolation, signatures_required }` |
| `is_alive()`, `shutdown()` | liveness, and an orderly stop |

---

[← Documentation index](../README.md#documentation)

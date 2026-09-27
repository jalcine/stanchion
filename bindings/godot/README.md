# stanchion-godot

Godot 4 (GDExtension) bindings for [stanchion](../../README.md): sandboxed Lua plugins
with capabilities and signatures, driven from GDScript.

Like the Python, Ruby and Kotlin/Swift shims, this is a thin translation layer over
`stanchion-ffi` — Godot `Variant`s in, plugin values out, GDScript `Callable`s wrapped
as capability providers and policies. Everything that decides *behaviour* lives in
`stanchion-ffi`, so the bindings cannot drift apart.

## Building

```sh
cargo build -p stanchion-godot              # debug  -> target/debug/libstanchion_godot.*
cargo build -p stanchion-godot --release    # release
```

Point a `.gdextension` file at the built library (see
[`examples/chess/stanchion.gdextension`](../../examples/chess/stanchion.gdextension)),
with `entry_symbol = "gdext_rust_init"` and `compatibility_minimum = 4.2`.

Lua 5.4 is built from source by default (`vendored`), so the extension ships without a
system Lua. Turn `default-features` off and pick `lua53`, `luajit` or `luau` to link a
different interpreter.

## Using it from GDScript

```gdscript
var s := Stanchion.new()

# Optional: answer a capability with a GDScript Callable. Register before open().
s.register_capability("log", func(call):
	print("[plugin %s] %s" % [call.plugin, call.args])
	return null)

# Build the sandbox, then load every plugin under a directory.
s.open({
	"plugins": ProjectSettings.globalize_path("res://plugins"),
	"instruction_limit": 200000,
})
var report := s.load("")           # "" == the configured root
print(report.loaded)               # ["greeter", ...]

# Call one method on one plugin; args and result are ordinary Variants.
var greeting = s.call("greeter", "greet", ["world"])

# Or ask every plugin the same thing at once.
for outcome in s.dispatch("handle", ["click"]):
	print(outcome.plugin, outcome.value, outcome.error)
```

### The `Stanchion` class

| Method | Returns | Notes |
| --- | --- | --- |
| `register_capability(name, callable)` | — | Before `open`. Callable gets `{plugin, capability, grant, args}`. |
| `set_policy(callable)` | — | Before `open`. See below for the answer shape. |
| `open(config)` | `bool` | Config keys: `plugins`, `shared`, `require_signatures`, `libs`/`deny`/`allow`, `memory_limit`, `instruction_limit`. |
| `load(root)` | `Dictionary` | `{loaded, failures, clean}`; `""` means the configured root. |
| `audit(root)` | `Array` | What each plugin requests, without running it. |
| `list()` / `names()` | `Array` / `PackedStringArray` | Loaded plugins. |
| `call(plugin, method, args)` | `Variant` | `null` on error. |
| `dispatch(method, args)` | `Array` | One `{plugin, value, error}` per plugin. |
| `reload(plugin)` / `revoke(plugin, capability)` | `bool` | |
| `isolation()` | `String` | `"shared"` or `"per-plugin"`. |
| `count()` / `is_open()` / `get_last_error()` | | Errors are also printed via `push_error`. |

A **policy** callable is handed `{plugin, capability, params, optional, signer}` and
returns one of: `true` (grant), `false` or a `String` (deny with that reason),
`{ "grant_with": params }` (grant with substituted parameters), or `{ "deny": reason }`.

Errors do not raise in GDScript — a failed call records a message (retrievable with
`get_last_error()`, and echoed to the Godot log) and returns a null/empty value.

## Threading

A Godot `Callable` is bound to the engine and is not `Send`. Use a `Stanchion` from the
main thread only; do not share it across `WorkerThreadPool` or a `Thread`.

## Tests

```sh
cargo test -p stanchion-godot
```

`tests/combinations.rs` loads the chess-combination plugins from
[`examples/chess`](../../examples/chess) through the real runtime and checks each one
picks the move it should — the Godot-free half of that example's test story.

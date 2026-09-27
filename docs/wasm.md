# WASM plugins

Stanchion has an **experimental** WASM backend that loads WASM plugins alongside Lua
ones. Both run in the same registry and are reached through the same
[`Stanchion`](../bindings.md) API, with the manifest's `plugin_type` field selecting
the runtime.

It is early and deliberately minimal. Today it marshals only scalar numbers across the
boundary — an export takes and returns `i32`/`i64`/`f32`/`f64` and nothing else. There
is no string, bytes, list or map marshalling yet (those need linear-memory access that
is not wired up), so a contract like the `String`-typed greeter used elsewhere in these
guides cannot run under WASM as written. Lua remains the fully supported runtime; reach
for WASM when a plugin's interface is numeric, or to experiment.

## Quick start

```toml
# plugins/my_wasm_plugin/plugin.toml
name = "my_wasm_plugin"
version = "0.1.0"
plugin_type = "wasm"
entry = "plugin.wasm"
```

Register the WASM backend before building the host:

```rust
use std::sync::Arc;
use stanchion_ffi::{Stanchion, PluginInstance};
use stanchion_wasm::WasmBackend;

Stanchion::builder()
    .backend(Box::new(WasmBackend::new()))
    .build()?;
```

## Manifest fields

| Field          | Description                                                           |
|----------------|-----------------------------------------------------------------------|
| `plugin_type`  | `"wasm"` selects the WASM backend                                     |
| `entry`        | Path to the `.wasm` binary, relative to the plugin directory           |

## The WASM interface

A WASM plugin exports functions by name. Arguments and return values use the same
[`Value`] type that Lua plugins use, but only the scalar numeric variants cross the
boundary today. An out-of-range `Int` (one that does not fit the target `i32`) is
rejected rather than silently truncated, and anything unsupported is refused with an
error rather than passed as a wrong value:

| `Value` variant | WASM type | Status |
|-----------------|-----------|--------|
| `Int`           | `i32` / `i64` | supported |
| `Float`         | `f32` / `f64` | supported |
| `Nil`           | — (no return value) | supported as an empty result |
| `Str`           | needs linear-memory marshalling | not supported yet |
| `Bool`          | — | not supported |
| `List` / `Map`  | needs linear-memory marshalling | not supported yet |

A call whose argument count does not match the export's signature, or that passes an
unsupported variant, fails before the function runs.

## Backend architecture

The [`PluginBackend`] trait makes the dispatch extensible:

```rust
pub trait PluginBackend: Send + Sync {
    fn plugin_type(&self) -> PluginType;
    fn load(&self, manifest: &Manifest, dir: &Path)
        -> Result<Box<dyn PluginInstance>>;
}
```

The Lua backend is built-in and registered automatically; the WASM backend is the one
other implementation that ships today. The trait is the seam a future runtime would
plug into — implement it and register the backend with [`Builder::backend`].

## Isolation

WASM plugins run in their own sandboxed instance via [wasmtime], under host-set
resource limits ([`WasmLimits`]): a per-call fuel ceiling, so an infinite loop traps
instead of hanging the calling thread, and a linear-memory ceiling enforced by a
store limiter. The limits are the host's — registered on the backend with
[`WasmBackend::with_limits`] — and a plugin manifest's `budget.max_instructions` may
only *lower* the fuel ceiling, never raise it. `WasmBackend::new()` uses secure
defaults (1e9 fuel, 64 MiB). Plugins cannot reach the host filesystem unless WASI
capabilities are explicitly granted through the capability system.

[`WasmLimits`]: https://docs.rs/stanchion-wasm/latest/stanchion_wasm/struct.WasmLimits.html
[`WasmBackend::with_limits`]: https://docs.rs/stanchion-wasm/latest/stanchion_wasm/struct.WasmBackend.html#method.with_limits

[`Stanchion`]: https://docs.rs/stanchion-ffi/latest/stanchion_ffi/struct.Stanchion.html
[`Value`]: https://docs.rs/stanchion-ffi/latest/stanchion_ffi/enum.Value.html
[`PluginBackend`]: https://docs.rs/stanchion-ffi/latest/stanchion_ffi/trait.PluginBackend.html
[`Builder::backend`]: https://docs.rs/stanchion-ffi/latest/stanchion_ffi/struct.Builder.html#method.backend
[wasmtime]: https://wasmtime.dev/
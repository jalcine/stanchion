# WASM plugins

Stanchion supports WASM-based plugins alongside Lua plugins. Both run in the same
registry and are callable through the same [`Stanchion`](../bindings.md) API, with
the manifest's `plugin_type` field selecting the runtime.

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
    .backend(Box::new(WasmBackend))
    .build()?;
```

## Manifest fields

| Field          | Description                                                           |
|----------------|-----------------------------------------------------------------------|
| `plugin_type`  | `"wasm"` selects the WASM backend                                     |
| `entry`        | Path to the `.wasm` binary, relative to the plugin directory           |

## The WASM interface

A WASM plugin exports functions by name. Arguments and return values use the same
[`Value`] type that Lua plugins use, with these constraints:

| Value type  | WASM type |
|-------------|-----------|
| `Int`       | `i32` / `i64` |
| `Float`     | `f32` / `f64` |
| `Str`       | Requires memory access (advanced) |
| `Nil`       | No return value |
| `Bool`      | Not supported |
| `Bytes`     | Not supported |
| `List` / `Map` | Not yet supported |

## Backend architecture

The [`PluginBackend`] trait makes the dispatch extensible:

```rust
pub trait PluginBackend: Send + Sync {
    fn plugin_type(&self) -> PluginType;
    fn load(&self, manifest: &Manifest, dir: &Path)
        -> Result<Box<dyn PluginInstance>>;
}
```

Implement it for any runtime — WASM, Python, JavaScript — and register it with
[`Builder::backend`]. The Lua backend is built-in and registered automatically.

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
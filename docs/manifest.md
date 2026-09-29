# Plugin manifests

Every plugin ships a `plugin.toml`. It is the static declaration a plugin makes
about itself — name, version, runtime, what it depends on, and what it wants to
reach — read *before* any of the plugin's own code runs.

A plugin is a directory. Discovery walks a plugin root and treats every immediate
subdirectory that contains a `plugin.toml` as a plugin; a directory without one is
skipped. The manifest types live in [`stanchion-abi`](https://github.com/jalcine/stanchion/tree/main/crates/stanchion-abi)
so a WASM-only build can read a manifest without linking Lua, while discovery and
[dependency ordering](dependencies.md) live in the registry.

```toml
name = "greeter"
version = "1.2.0"
plugin_type = "lua"
entry = "init.lua"

[dependencies]
formatter = "^1.0"
logger = { version = "^2.0", optional = true }

[capabilities.network]
hosts = ["api.example.com"]

[config]
greeting = "hello"

[budget]
max_instructions = 5_000_000
```

The manifest is parsed with **unknown fields rejected**: a typo like `pluginType`
or `dependancies` fails the plugin at discovery rather than being silently ignored.

## Fields

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `name` | string | *required* | Unique identifier. Dispatch, reload and dependencies all address a plugin by this. |
| `version` | semver | `0.0.0` | The version other plugins match requirements against. Absent means `0.0.0`, which satisfies only `*`. |
| `plugin_type` | `"lua"` \| `"wasm"` | `"lua"` | Which runtime backend loads the plugin. |
| `entry` | path | `"init.lua"` | File to evaluate, relative to the plugin directory. |
| `[dependencies]` | table | empty | Other plugins this one is wired to, by name. See [dependency chains](dependencies.md). |
| `[capabilities]` | table of tables | empty | Capabilities requested, as name to parameters. See [capabilities](capabilities.md). |
| `[rocks]` | table | empty | LuaRocks packages needed, as name to requirement. See [LuaRocks](luarocks.md). |
| `[config]` | table | empty | Passed to the plugin's constructor as a Lua table. |
| `[budget]` | table | none | Optional per-plugin instruction budget. |

### `name`

The name flows into filesystem paths (`root/<name>`, staging directories), log
lines, remote-protocol messages and error strings, so it is held to a strict,
portable grammar and validated at the earliest point it is read:

- 1–128 characters,
- ASCII letters, digits, `-`, `_` and `.` only,
- never `.` or `..` on its own.

So `my-plugin`, `weather_v2`, `a.b.c` and `30log` are fine; `a/b`, `../evil`,
`has space` and an empty name are refused. This is the same grammar the
[distribution index](distribution.md) enforces, so a name is checked identically
whether it came from a manifest or an index lookup.

### `version`

A [semver](https://docs.rs/semver) version. It deserializes straight from the
manifest, so a malformed version is a manifest error rather than a runtime
surprise. A plugin without a `version` is treated as `0.0.0` — it satisfies a
dependent's `*` requirement and nothing narrower.

### `plugin_type` and `entry`

`plugin_type` selects the backend: `"lua"` (the default, loaded via `mlua`) or
`"wasm"` (loaded via `wasmtime` — see [WASM plugins](wasm.md)). `entry` names the
file to evaluate, relative to the plugin directory, and its extension must match
the backend: a Lua entry ends with `.lua`, a WASM entry with `.wasm`.

`entry` is confined to the plugin directory. It must be a relative path with no
`..` component and no absolute or root prefix, so it cannot point at
`../elsewhere/x.wasm` or `/etc/init.lua` — a path the plugin's digest would never
cover. Subdirectories are allowed (`src/init.lua`, `./build/plugin.wasm`).

### `[dependencies]`

Each entry is another plugin's name mapped to a semver requirement, in either of
two forms:

```toml
[dependencies]
formatter = "^1.0"                              # bare requirement
logger    = { version = "^2.0", optional = true }
```

A required dependency that is missing, or present at an unsatisfying version, fails
the plugin before any code runs (`MissingDependency` / `IncompatibleDependency`).
An `optional` dependency that is absent leaves a `nil` slot instead of failing.
Only direct dependencies are injected, and a dependency has to be discovered in the
same plugin root. The full mechanics — published exports, reload propagation, and
how dependencies interact with isolation — are in [dependency chains](dependencies.md).

### `[capabilities]`

Each subtable is a capability the plugin requests, keyed by name, with parameters
as the table body:

```toml
[capabilities.network]
hosts = ["api.example.com"]

[capabilities.metrics]
optional = true          # reserved key, stripped before the provider sees params
```

`optional` is reserved: the registry strips it and passes everything else to the
host's provider once policy has approved the request. Declaring a capability only
*asks*; the host's providers and [policy](capabilities.md) decide what is actually
granted, and everything is deny-by-default. Because `[capabilities]` can be read
without running the plugin, `audit` can report what a plugin wants before you
accept it.

### `[rocks]`

External Lua libraries, as package name to a LuaRocks requirement:

```toml
[rocks]
dkjson = "2.11"
lpeg   = ">= 1.0, < 2.0"
```

Rock requirements are **not** semver — they use LuaRocks' own operators and are
checked against a configured tree. Manifests may always carry `[rocks]` so they
stay portable, but the declarations are only acted on with the `luarocks` feature.
See [LuaRocks](luarocks.md).

### `[config]`

An arbitrary table handed to the plugin's constructor. For a Lua class plugin it
arrives as the `config` argument to `new(config, deps)`:

```lua
function Formatter.new(config)
  return setmetatable({ prefix = config.prefix or "" }, Formatter)
end
```

### `[budget]`

An optional per-plugin instruction cap, enforced at each call boundary for runtimes
that support it (WASM, the Lua sandbox):

```toml
[budget]
max_instructions = 5_000_000
```

Like the rest of the manifest, the `[budget]` table rejects unknown keys, so
`max_instructions` is the only field it accepts.

## Discovery and validation

`discover` reads every `<root>/*/plugin.toml` in sorted order, so load order is
reproducible regardless of how the filesystem hands back directory entries. A
directory whose manifest is unreadable or invalid becomes a reported load failure
rather than aborting discovery of the rest.

A manifest is validated up front — the name grammar, the entry path, and the
entry's extension against `plugin_type` — so a plugin that cannot possibly load is
rejected while you are still only reading manifests, never mid-run.

## Minimal manifest

Only `name` is required. The smallest usable Lua plugin is a directory holding:

```toml
# plugin.toml
name = "grumpy"
version = "0.1.0"
```

with an `init.lua` beside it. Everything else — `plugin_type`, `entry`,
dependencies, capabilities, config, budget — takes its default.

---

[← Documentation index](../README.md#documentation)

# Isolation

Whether plugins share one Lua state or get one each, and what a sandbox
policy can bound.

Three modes, chosen at construction.

```rust
// Shared: every plugin runs in the state you hand over.
let registry = Registry::new(Lua::new());

let policy = Sandbox::restricted()
    .memory_limit(16 * 1024 * 1024)
    .instruction_limit(5_000_000);

// Per-plugin: every plugin gets its own state under a policy.
let registry = Registry::isolated(Lua::new(), policy.clone());

// Per-group: plugins wired together by `[dependencies]` share a state.
let registry = Registry::grouped(Lua::new(), policy);
```

| | Shared | Per-group | Per-plugin |
| --- | --- | --- | --- |
| Memory / instruction limits | impossible | per group | per plugin |
| Standard library selection | one policy, set by you | per registry, via `Sandbox` | per registry, via `Sandbox` |
| `[dependencies]` exports | injected | injected within a group | **unavailable** — values cannot cross states |
| Blast radius of a bad plugin | shared globals and allocator | its group's globals and allocator | contained to its own state |
| Cost | one state | one state per group | one state per plugin |

Under **shared** isolation each chunk is evaluated with its own environment table whose
`__index` is the real globals: a plugin **reads** globals normally but its **writes**
stay local, so one plugin cannot redefine `string.format` for the others. That is
namespace hygiene, not a security boundary — `rawset(_G, ...)` still reaches the shared
state, and nothing bounds CPU or memory.

Under **per-plugin** isolation the boundary is real. Memory and instruction limits are
properties of a `Lua`, which is exactly why they cannot be applied inside a shared
state. The price is that Lua values cannot cross states, so a `[dependencies]` entry
that would inject exports fails with `CrossStateDependency` (see
[dependency chains](dependencies.md)).

## Per-group: limits without giving up dependencies

`Registry::grouped` pays that price only where it is unavoidable. A Lua value cannot
cross states, so the registry puts the plugins that need to exchange values in the same
one: each **connected component** of the dependency graph gets a state, and a plugin
depending on nothing gets one to itself.

```
base ←── middle ←── top        island
└──────── one state ──────┘    └─ own ─┘
```

Connectivity is undirected, so a diamond — `left` and `right` both on `shared`, `top` on
both — is one group rather than two that later discover they must merge. The partition is
computed over the whole graph before any state is built.

`Plugin::group()` reports which state a plugin ended up in; two plugins reporting the
same `Group` share one. The group is the accounting unit: members share a heap, a memory
limit and one instruction budget, so `Budget::used` reads the same for all of them.

What this does not do is make a dependency free. Group members share globals and an
allocator, exactly as plugins do under shared isolation — a runaway loop in one exhausts
the group's allowance for all of them. Declaring a dependency is declaring that you
accept that. What you get is that the blast radius is the group rather than the process,
and that groups are as separated from each other as plugins are under per-plugin
isolation.

## Panics, and what cannot be caught

A Rust panic raised inside one of your callbacks is survivable, because a panic
unwinds. It never crosses the interpreter's C frames — mlua wraps every callback in
`catch_unwind` and carries the payload out as a Lua error, resuming it once control is
back in Rust — and `dispatch`, `dispatch_async`, `load_dir` and `reload` catch that
resumption, so it arrives as a `Panicked` error or a `FailureReason::Panicked` rather
than on the caller's stack. Recover it by type with `mlua::Error::downcast_ref`, not
with a `source()` walk.

Being catchable is not the same as being harmless. mlua makes no promise about a state's
invariants after a caught panic, so treat the plugin as suspect and unload it with
`Registry::remove` rather than dispatching to it again. Two further limits: a profile
built with `panic = "abort"` ends the process before any of this runs, and
`Sandbox::catch_rust_panics` is left at Lua's default, which means a plugin that wraps a
host call in `pcall` can swallow your panic before you ever see it — pass `false` if
that matters more than compatibility with plugins already relying on it.

Nothing in-process helps with a failure that does not unwind. `os.exit` calls `exit()`;
a segfault in a C rock, LuaJIT `ffi` misuse, or a Rust stack overflow raises a signal. A
handler could log one but cannot resume: the allocator, the VM's invariants and any held
locks are undefined afterwards, and signal handlers are process-global, so a plugin
library installing one would hijack the host's own crash reporting. The process boundary
is the only real answer — see [out-of-process hosting](remote.md).

Each plugin's directory is prepended to its state's `package.path`, so a plugin can
`require` its own files without knowing where it was installed. That `require` is
per-plugin: submodules load into the **same environment** as the plugin's own chunk, so
they see its granted capabilities, and each plugin gets its own module cache rather
than sharing `package.loaded`.

## Sandbox policy

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

## Reaching an isolated plugin

A freshly created state has nothing of yours in it. Host functions get there through
[capabilities](capabilities.md), declared in `with_setup` and granted per plugin by
policy — so what a plugin can reach stays a decision rather than a side effect of
being loaded.

---

[← Documentation index](../README.md#documentation)

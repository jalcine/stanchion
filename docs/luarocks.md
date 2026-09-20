# LuaRocks

Declaring external Lua libraries in a manifest and making them loadable.

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

## C modules

`package.cpath` is left alone unless you opt in:

```rust
RocksConfig::new(tree).load_c_modules(true)
```

A C rock is a shared object linked against some `liblua`. With `vendored`, mlua links
Lua statically into the host binary, so loading one can pull a **second Lua runtime**
into the process — a crash or silent corruption, not a clean error. Pure-Lua rocks are
unaffected. Only enable this when mlua links against the same external Lua the rocks
were built against.

---

[← Documentation index](../README.md#documentation)

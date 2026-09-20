# Dependency chains

Plugins that use each other: published exports, semver requirements, and
how a [reload](registry.md#reload) propagates through a chain.

# Dependency chains

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

---

[← Documentation index](../README.md#documentation)

# Dependency chains

Plugins that use each other: published exports, semver requirements, and
how a [reload](registry.md#reload) propagates through a chain.

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

## Dependencies and isolation

An injected export is a Lua value, and a Lua value cannot cross states. So dependencies
work under shared isolation, work within a group under
[`Registry::grouped`](isolation.md#per-group-limits-without-giving-up-dependencies), and
fail with `CrossStateDependency` under `Registry::isolated`.

Grouped isolation is the middle setting worth knowing about: plugins connected by a
chain of dependencies share one sandboxed state, and get memory and instruction limits
as a group, while plugins with nothing between them stay apart. Declaring a dependency
is therefore also declaring that you accept sharing a heap and a budget with that plugin.

A dependency also has to be discovered in the same `load_dir` call as the plugin that
declares it: the requirement is matched against what that root holds, so a plugin whose
dependency lives elsewhere fails with `MissingDependency`.

---

[← Documentation index](../README.md#documentation)

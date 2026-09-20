# Capabilities

Plugins declare what they need; the host declares what it offers; a policy
decides what is actually granted.

# Capabilities

`with_setup` is where the host declares **everything** a plugin can reach. Capabilities
are gated; ambient values are not, and both are reported by `audit`.

```rust
let registry = Registry::isolated(Lua::new(), Sandbox::restricted())
    .with_setup(|host| {
        host.capability("network", |lua, grant| {
            // The allowlist is baked into the value, so the plugin cannot widen it.
            let allowed: Vec<String> = grant.get_or_default("hosts");
            Ok(Value::Function(lua.create_function(move |_, url: String| {
                if allowed.contains(&url) { fetch(&url) } else { Err(denied()) }
            })?))
        });
        host.ambient("HOST_VERSION", |lua| lua.globals().set("HOST_VERSION", "1.0"));
        Ok(())
    })
    .with_policy(Rules::deny_all().allow_with("network", narrow_to_one_host));
```

A plugin requests what it needs in its manifest:

```toml
[capabilities.network]
hosts = ["api.example.com"]

[capabilities.metrics]
optional = true          # reserved key; stripped before the provider sees params
```

Granted names are bound in that plugin's environment, so the class contract is
untouched — constructors stay `new(config, deps)`:

```lua
function Weather:fetch()
  return network(self.host)   -- nil if never granted
end
```

**Deny by default.** Registering a provider says the host *can* offer something;
the policy decides who *gets* it. Without a policy every capability is refused, even
one with a provider.

**The policy can narrow, not just refuse.** A host that can only answer yes or no to
`hosts = ["*"]` is a rubber stamp. `Decision::GrantWith` replaces the requested
parameters, and the provider closes over the approved ones — so there is no parameter
left for the plugin to tamper with at call time.

| Outcome | Required capability | Optional capability |
| --- | --- | --- |
| Policy grants | bound in the environment | bound in the environment |
| Policy denies | `CapabilityDenied`, plugin fails | left unbound, plugin loads |
| No provider offered | `UnknownCapability`, plugin fails | left unbound, plugin loads |

## Audit, introspection, revocation

```rust
let audit = registry.audit("plugins/")?;      // executes nothing
for request in audit.unsatisfiable() { /* asks for what the host cannot offer */ }
audit.ambient;                                 // reaches every plugin regardless

plugin.granted_capabilities();                 // what policy actually allowed
registry.revoke("weather", "network")?;        // unbind on a live plugin
```

`audit` reads manifests only, so you can review what a plugin wants **before running
any of its code** — the precondition for accepting plugins you did not write. Revoking
sets the name to `nil` in the plugin's environment; code that already captured the
value in a local keeps it, so revocation defangs a plugin rather than rewinding it.

## What this does not buy

- **A sloppy provider defeats it.** If a provider ignores its `Grant` and reads any
  path, the declaration is a comment. Enforcement lives in the provider.
- **It bounds reach, not use.** A plugin granted one host can still hammer that host.
- **Manifests are self-asserted.** A modest declaration only means something once
  signatures make the manifest attributable.

---

[← Documentation index](../README.md#documentation)

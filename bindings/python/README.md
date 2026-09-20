# stanchion

Lua plugins for Python applications: sandboxing, capabilities and signatures.

```python
import stanchion

host = stanchion.Stanchion(capabilities={"kv": lambda call: store[call.args[0]]})
report = host.load("plugins/")
print(host.call("greeter", "greet", "world"))
```

Ships with Lua 5.4 built from source, so nothing needs to be installed on the system.

See [docs/bindings.md](https://github.com/jalcine/stanchion/blob/main/docs/bindings.md).

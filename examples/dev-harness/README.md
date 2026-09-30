# dev-harness

A development harness: watch a plugin directory, reload on change, smoke-test.

```sh
cargo run -q -p dev-harness --bin harness-watch -- ./plugins counter
```

Edit `plugins/counter/plugin.toml` (the `message`) or `init.lua` and save: the
harness reloads and shows the new output without restarting. Break `init.lua`
and the reload is refused — the previous instance keeps serving, which the
smoke call after every reload demonstrates.

The loop polls the directory digest twice a second. No file-watching
dependency, nothing platform-specific; the digest comparison also means only
real changes reload, not editor save noise that leaves bytes identical.

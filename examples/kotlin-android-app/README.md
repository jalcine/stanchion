# Stanchion Android demo

A minimal Android app showing how Lua plugins drive app behavior through
the UniFFI Kotlin bindings (`bindings/uniffi`):

- **Themes** — `theme_*` plugins return colors + fonts; the UI switches with no recompile.
- **Transposable text** — `strings_*` plugins translate keys; Kotlin interpolates `{name}`.
- **Layout** — `layout_compact` returns section order.
- **Flags** — `flags` returns feature toggles, gated by an in-app `Policy`.
- **Capability** — a Kotlin `LocaleProvider` answers the `locale` capability plugins can call.

## Build

```sh
mise install          # SDK, NDK driver, Gradle, Kotlin, ktlint
mise run android:build  # .so per ABI + UniFFI Kotlin (into app/build) + assembleDebug
mise run android:lint   # ktlint
```

The generated Kotlin and `.so` files live under `app/build/` and are never
committed. `app/build.gradle.kts` also regenerates the bindings via
`generateStanchionBindings` on every `preBuild`, so plain `gradle assembleDebug`
works once the NDK platform is installed.

Install with `adb install app/build/outputs/apk/debug/app-debug.apk`.

## Plugin contract

Each folder under `app/src/main/assets/plugins/` is one plugin
(`plugin.toml` + `init.lua`). Methods resolve by name at call time:

| Plugin | Method | Returns |
| --- | --- | --- |
| `theme_ocean`, `theme_forest` | `theme()` | `{name, colors={background, surface, primary}, fonts={body, display, scale}}` |
| `strings_en`, `strings_es` | `text(key)` | translated string, or `nil` |
| `layout_compact` | `layout()` | `{order=[...]}` |
| `flags` | `flags()` | `{newHeader=bool}` |

Plugins may call `locale()` (the `locale` capability). It is ungranted by
default, so plugins `pcall` it and fall back — fail-closed. A broken plugin
shows up in `LoadReport.failures` and the app keeps its built-in defaults.

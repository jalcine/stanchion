# Testing

Tests run under [cargo-nextest](https://nexte.st). Each test gets its own process,
which suits this workspace: several tests spawn child processes, shell out to
`luarocks`, or drive Lua states into their memory and instruction limits, and one
wedged test cannot take the others with it.

```sh
mise run test
```

**Run it through `mise`, not `cargo` directly.** The feature set is the part that is
easy to get wrong, so it lives once in `$STANCHION_FEATURES` in
[`mise.toml`](../mise.toml) and every task — the suite, the linter, the doc build —
reads it from there. A hand-written `--features` list is how this goes wrong twice: it
drifts from what CI checks, and the drift is invisible because a narrower run still
reports success.

Which is the real hazard. Integration tests are gated per feature, so a run missing
them compiles most of the suite away and passes:

```sh
cargo nextest run --workspace --features lua54,vendored   # 173 tests, all green
mise run test                                             # 292 tests, all green
```

Both of those say "0 failed". Only one of them tested the thing you changed.

Three profiles are configured in [`.config/nextest.toml`](../.config/nextest.toml):

| Profile | For |
| --- | --- |
| `default` | everything; slow tests are reported and then killed |
| `quick` | iteration — skips the `rocks` and `ui` suites, which pay for an external tool and a compiler round-trip |
| `ci` | retries the two suites that depend on an external tool or a real child process, and writes `junit.xml` |

```sh
mise exec -- cargo nextest run -P quick --workspace --features "$STANCHION_FEATURES"
```

(`$STANCHION_FEATURES` is set by `mise.toml`, so reaching for `cargo` directly means
going through `mise exec` — or having mise activated in the shell — rather than
writing the feature list out again.)

A `slow-timeout` is deliberate rather than decorative: several tests prove that a
runaway plugin gets stopped — an endless loop, an allocation storm, a process that
kills itself. If one of those guards regressed, the test would otherwise hang forever,
so nextest reports it slow and then terminates it.

**nextest does not run doctests.** Run those separately:

```sh
mise run test:doc
```

The `luarocks` tests build a rock offline with `luarocks make` from a local rockspec,
so they need no network, and they skip themselves if `luarocks` is not on `PATH`. They
are the slowest tests here, so they are limited to one at a time.

`crates/stanchion/tests/ui/` holds `trybuild` compile-fail cases pinning the macro's
diagnostics. They only assert on `error:` lines the macro itself emits, so they are not
sensitive to rustc version. After deliberately changing a message, refresh the
expectations with:

```sh
TRYBUILD=overwrite cargo test -p stanchion --features lua54,vendored --test ui
```

A mismatch writes the actual output to `wip/` for comparison. (Refreshing uses
`cargo test`, because `TRYBUILD=overwrite` rewrites files as a side effect of the run.)

## Examples

The examples under [`crates/stanchion/examples/`](../crates/stanchion/examples)
compile as part of an ordinary build, so they cannot rot into documentation that no
longer works:

```sh
mise exec -- cargo build --workspace --features "$STANCHION_FEATURES" --examples
```

`--workspace` rather than `-p stanchion`, because `$STANCHION_FEATURES` names features
of `stanchion-ffi` and `stanchion-index` too, and a package-scoped build rejects
features that are not that package's own.

Each declares `required-features`, so a build with a narrower feature set skips
the ones it cannot compile rather than failing — quietly, which is the same hazard as
above.

## The rest of the gates

Each of these is a CI job and a single command locally:

| Command | What it holds |
| --- | --- |
| `mise run test` | the suite, under nextest |
| `mise run test:doc` | doctests, which nextest does not run |
| `mise run lint` | clippy over the workspace and the binding crates, warnings denied |
| `mise run lint:docs` | rustdoc with warnings denied — catches a broken intra-doc link, which is how documentation quietly stops describing the code |
| `mise run lint:deps` | [cargo-deny](https://embarkstudios.github.io/cargo-deny/): advisories, licences and sources over the dependency graph. See [`deny.toml`](../deny.toml), where every ignored advisory carries why it does not apply here and when to drop the entry |
| `mise run lint:msrv` | builds against the declared MSRV rather than whatever is installed |
| `mise run miri` | [Miri](https://github.com/rust-lang/miri) over the crates whose `unsafe` is ordinary Rust. The FFI crates are excluded: their `unsafe` *is* a foreign boundary, which Miri cannot model |
| `mise run fuzz` | short [cargo-fuzz](https://rust-fuzz.github.io/book/cargo-fuzz.html) runs over the two places untrusted bytes enter — a downloaded `tar.gz` and a `plugin.toml`. Seeded from `fuzz/seeds/`, which matters: random bytes almost never form a valid gzip member, so an unseeded run never reaches the archive checks it exists to test |
| `mise run smoke` | every language binding, end to end — Python, Ruby, Kotlin and the UniFFI boundary |

`fuzz` and `miri` need a nightly toolchain and `cargo-fuzz` installed; the rest run on
the pinned stable. A real fuzzing campaign is the same command without
`-max_total_time`, for hours rather than a minute.

## Coverage

Code coverage can be generated using [cargo-tarpaulin](https://github.com/xd009642/tarpaulin):

```sh
mise run test:coverage
```

This runs the full test suite and generates:
- HTML report: `target/coverage/index.html`
- Lcov report: `target/coverage/lcov.info` (for tools like Codecov)

For faster iteration (excludes slow tests like luarocks and UI tests):

```sh
mise run test:coverage:quick
```

The coverage job runs automatically in CI and produces an artifact named `coverage`.

---

[← Documentation index](../README.md#documentation)

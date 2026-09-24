# Testing

Tests run under [cargo-nextest](https://nexte.st). Each test gets its own process,
which suits this workspace: several tests spawn child processes, shell out to
`luarocks`, or drive Lua states into their memory and instruction limits, and one
wedged test cannot take the others with it.

```sh
cargo nextest run --workspace --features lua54,vendored,full
```

The `full` feature matters. Integration tests are gated per feature, so a run without
it compiles most of the suite away and still reports success — 22 tests instead of 103.

Three profiles are configured in [`.config/nextest.toml`](../.config/nextest.toml):

| Profile | For |
| --- | --- |
| `default` | everything; slow tests are reported and then killed |
| `quick` | iteration — skips the `rocks` and `ui` suites, which pay for an external tool and a compiler round-trip |
| `ci` | retries the two suites that depend on an external tool or a real child process, and writes `junit.xml` |

```sh
cargo nextest run -P quick --workspace --features lua54,vendored,full
```

A `slow-timeout` is deliberate rather than decorative: several tests prove that a
runaway plugin gets stopped — an endless loop, an allocation storm, a process that
kills itself. If one of those guards regressed, the test would otherwise hang forever,
so nextest reports it slow and then terminates it.

**nextest does not run doctests.** Run those separately:

```sh
cargo test --doc --workspace --features lua54,vendored,full
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
cargo build -p stanchion --features lua54,vendored,full --examples
```

Each declares `required-features`, so a build with a narrower feature set skips
the ones it cannot compile rather than failing.

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

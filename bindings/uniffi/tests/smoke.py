"""Exercises the generated UniFFI boundary at runtime.

Compiling proves the types map. It does not prove that a plugin calling a foreign
capability reaches the foreign implementation, that throwing from one surfaces inside
Lua, or that re-entering the registry from a callback is caught rather than hanging.
All of that runs through UniFFI's generated scaffolding — a different mechanism from
the hand-written `pyo3` binding — so it needs its own test.

Python stands in for Kotlin and Swift here because it needs no toolchain beyond the
one already present. The scaffolding under test is the same in all three: the same
vtables, the same callback-interface machinery, the same error conversions.

Run through `bindings/uniffi/smoke.sh`, which builds the cdylib and regenerates the
bindings first.
"""

from __future__ import annotations

import asyncio
import sys
import tempfile
from pathlib import Path

from stanchion_uniffi import (
    CapabilityCall,
    CapabilityProvider,
    CapabilityRequest,
    Config,
    Decision,
    Policy,
    ProviderError,
    Stanchion,
    StanchionError,
    Value,
)

ECHO = """
local P = {}
P.__index = P
function P.new(config) return setmetatable({ tag = config.tag or "echo" }, P) end
function P:identity(value) return value end
function P:tagged() return self.tag end
function P:boom() error("deliberate") end
return P
"""

CALLER = """
local P = {}
P.__index = P
function P.new(_) return setmetatable({}, P) end
function P:lookup(key) return kv(key) end
function P:attempt(key)
  local ok, err = pcall(function() return kv(key) end)
  return ok and err or ("refused: " .. tostring(err))
end
return P
"""

YIELDING = """
local P = {}
P.__index = P
function P.new(_) return setmetatable({}, P) end
function P:tagged() coroutine.yield() return "awake" end
return P
"""

failures: list[str] = []


def check(name: str, condition: bool, detail: str = "") -> None:
    if condition:
        print(f"  ok   {name}")
    else:
        print(f"  FAIL {name} {detail}")
        failures.append(name)


def write_plugin(root: Path, name: str, manifest: str, source: str) -> None:
    directory = root / name
    directory.mkdir(parents=True, exist_ok=True)
    (directory / "plugin.toml").write_text(manifest)
    (directory / "init.lua").write_text(source)


def config(**kwargs: object) -> Config:
    return Config(
        plugins=kwargs.get("plugins"),
        libs=None,
        deny=None,
        memory_limit=None,
        instruction_limit=None,
        shared=bool(kwargs.get("shared", False)),
        require_signatures=False,
        allow=list(kwargs.get("allow", [])),  # type: ignore[arg-type]
    )


def test_values_round_trip(root: Path) -> None:
    print("values round-trip through the generated converters")
    write_plugin(root, "echo", 'name = "echo"\nentry = "init.lua"\n', ECHO)
    host = Stanchion(config(), {}, None)
    host.load(str(root))

    cases = [
        Value.NIL(),
        Value.BOOL(True),
        Value.INT(-9),
        Value.FLOAT(1.5),
        Value.STR("hello"),
        Value.SEQ([Value.INT(1), Value.INT(2)]),
        Value.TABLE({"k": Value.BOOL(False)}),
    ]
    for case in cases:
        got = host.call("echo", "identity", [case])
        check(f"identity {case}", got == case, f"got {got}")

    check("tagged", host.call("echo", "tagged", []) == Value.STR("echo"))
    check("count", host.count() == 1)
    check("names", host.names() == ["echo"])
    check("isolation", host.isolation() == "per-plugin")


def test_errors_map(root: Path) -> None:
    print("failures arrive as the right generated exception")
    write_plugin(root, "echo", 'name = "echo"\nentry = "init.lua"\n', ECHO)
    host = Stanchion(config(), {}, None)
    host.load(str(root))

    try:
        host.call("absent", "tagged", [])
        check("unknown plugin raises", False)
    except StanchionError.UnknownPlugin:
        check("unknown plugin raises", True)
    except BaseException as err:  # noqa: BLE001
        check("unknown plugin raises", False, f"got {type(err).__name__}")

    try:
        host.call("echo", "boom", [])
        check("lua error raises", False)
    except StanchionError.Lua as err:
        check("lua error raises", "deliberate" in str(err), str(err))
    except BaseException as err:  # noqa: BLE001
        check("lua error raises", False, f"got {type(err).__name__}")


class Store(CapabilityProvider):
    """A foreign capability implementation, called from inside Lua."""

    def __init__(self) -> None:
        self.seen: list[CapabilityCall] = []

    def invoke(self, call: CapabilityCall) -> Value:
        self.seen.append(call)
        key = call.args[0]
        if isinstance(key, Value.STR) and key.value == "boom":
            raise ProviderError.Refused(reason="no such key")
        return Value.STR(f"value-of-{key.value}")  # type: ignore[union-attr]


def test_capability_callback(root: Path) -> None:
    print("a plugin reaches a foreign capability")
    write_plugin(
        root, "caller", 'name = "caller"\nentry = "init.lua"\n\n[capabilities.kv]\n', CALLER
    )

    store = Store()
    host = Stanchion(config(), {"kv": store}, None)
    report = host.load(str(root))
    check("caller loaded", report.loaded == ["caller"], str(report.failures))

    got = host.call("caller", "lookup", [Value.STR("alpha")])
    check("callback returned", got == Value.STR("value-of-alpha"), str(got))
    check("provider saw one call", len(store.seen) == 1)
    check("provider saw the plugin", store.seen[0].plugin == "caller")
    check("provider saw the capability", store.seen[0].capability == "kv")

    # Throwing from a foreign provider must become a catchable Lua error.
    message = host.call("caller", "attempt", [Value.STR("boom")])
    check(
        "a throwing provider is catchable in Lua",
        isinstance(message, Value.STR)
        and "refused" in message.value
        and "no such key" in message.value,
        str(message),
    )


class Narrowing(Policy):
    """A foreign policy that narrows rather than answering yes or no."""

    def __init__(self) -> None:
        self.seen: list[CapabilityRequest] = []

    def decide(self, request: CapabilityRequest) -> Decision:
        self.seen.append(request)
        return Decision.GRANT_WITH(Value.TABLE({"keys": Value.SEQ([Value.STR("alpha")])}))


def test_policy_callback(root: Path) -> None:
    print("a foreign policy decides, and can narrow")
    write_plugin(
        root,
        "caller",
        'name = "caller"\nentry = "init.lua"\n\n[capabilities.kv]\nkeys = ["*"]\n',
        CALLER,
    )

    store, policy = Store(), Narrowing()
    host = Stanchion(config(), {"kv": store}, policy)
    report = host.load(str(root))
    check("loaded under policy", report.loaded == ["caller"], str(report.failures))
    check("policy was consulted", len(policy.seen) == 1)
    check("policy saw the request", policy.seen[0].capability == "kv")
    check("policy saw the signer", policy.seen[0].signer == "unsigned")

    host.call("caller", "lookup", [Value.STR("alpha")])
    # The provider must see what policy approved, not what the manifest asked for.
    check(
        "the narrowed grant reached the provider",
        store.seen[0].grant == Value.TABLE({"keys": Value.SEQ([Value.STR("alpha")])}),
        str(store.seen[0].grant),
    )


class Denying(Policy):
    def decide(self, request: CapabilityRequest) -> Decision:
        return Decision.DENY("not in this deployment")


def test_policy_denial(root: Path) -> None:
    print("a foreign policy can refuse")
    write_plugin(
        root, "caller", 'name = "caller"\nentry = "init.lua"\n\n[capabilities.kv]\n', CALLER
    )
    host = Stanchion(config(), {"kv": Store()}, Denying())
    report = host.load(str(root))

    check("denied plugin did not load", report.loaded == [], str(report.loaded))
    check(
        "the reason came through",
        any("not in this deployment" in f.reason for f in report.failures),
        str(report.failures),
    )


class Reenters(CapabilityProvider):
    """Calls back into the registry that invoked it."""

    def __init__(self) -> None:
        self.caught: list[BaseException] = []
        self.host: Stanchion | None = None

    def invoke(self, call: CapabilityCall) -> Value:
        assert self.host is not None
        try:
            self.host.call("caller", "lookup", [Value.STR("again")])
        except BaseException as err:  # noqa: BLE001 - recording it is the point
            self.caught.append(err)
            raise ProviderError.Refused(reason=str(err)) from err
        return Value.NIL()


def test_reentrancy_is_caught(root: Path) -> None:
    print("re-entering from a generated callback is caught, not hung")
    write_plugin(
        root, "caller", 'name = "caller"\nentry = "init.lua"\n\n[capabilities.kv]\n', CALLER
    )

    provider = Reenters()
    host = Stanchion(config(), {"kv": provider}, None)
    provider.host = host
    host.load(str(root))

    # Without the guard this never returns.
    try:
        host.call("caller", "lookup", [Value.STR("k")])
        check("re-entry refused", False, "the call unexpectedly succeeded")
    except StanchionError.Lua:
        check("re-entry refused", True)
    except BaseException as err:  # noqa: BLE001
        check("re-entry refused", False, f"got {type(err).__name__}")

    check("the provider saw one failure", len(provider.caught) == 1)
    check(
        "it was the reentrancy error",
        provider.caught and isinstance(provider.caught[0], StanchionError.Reentrant),
        str(provider.caught),
    )


async def test_async(root: Path) -> None:
    print("async crosses the generated boundary")
    write_plugin(root, "sleeper", 'name = "sleeper"\nentry = "init.lua"\n', YIELDING)
    host = Stanchion(config(), {}, None)
    host.load(str(root))

    got = await host.call_async("sleeper", "tagged", [])
    check("call_async", got == Value.STR("awake"), str(got))

    outcomes = await host.dispatch_async("tagged", [])
    check("dispatch_async", [o.value for o in outcomes] == [Value.STR("awake")], str(outcomes))


def main() -> int:
    tests = [
        test_values_round_trip,
        test_errors_map,
        test_capability_callback,
        test_policy_callback,
        test_policy_denial,
        test_reentrancy_is_caught,
    ]
    for test in tests:
        with tempfile.TemporaryDirectory() as directory:
            test(Path(directory))

    with tempfile.TemporaryDirectory() as directory:
        asyncio.run(test_async(Path(directory)))

    print()
    if failures:
        print(f"{len(failures)} check(s) failed: {', '.join(failures)}")
        return 1
    print("every check passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())

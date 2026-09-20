"""Python answering capabilities that Lua plugins call."""

from __future__ import annotations

from pathlib import Path

import pytest

import stanchion


def test_a_plugin_reaches_a_python_capability(caller_root: Path) -> None:
    seen: list[stanchion.CapabilityCall] = []

    def kv(call: stanchion.CapabilityCall) -> str:
        seen.append(call)
        return f"value-of-{call.args[0]}"

    host = stanchion.Stanchion(capabilities={"kv": kv})
    assert host.load(caller_root).is_clean

    assert host.call("caller", "lookup", "alpha") == "value-of-alpha"

    (call,) = seen
    assert call.plugin == "caller"
    assert call.capability == "kv"
    assert call.args == ["alpha"]


def test_a_provider_may_return_any_convertible_value(caller_root: Path) -> None:
    host = stanchion.Stanchion(capabilities={"kv": lambda _call: {"a": [1, True]}})
    host.load(caller_root)
    assert host.call("caller", "lookup", "k") == {"a": [1, True]}


def test_a_raising_provider_is_catchable_inside_lua(caller_root: Path) -> None:
    def kv(_call: stanchion.CapabilityCall) -> str:
        raise KeyError("no such key")

    host = stanchion.Stanchion(capabilities={"kv": kv})
    host.load(caller_root)

    message = host.call("caller", "attempt", "k")
    assert "refused" in message
    assert "no such key" in message


def test_a_capability_nobody_provides_keeps_the_plugin_out(caller_root: Path) -> None:
    # No provider and no allow-list: the default policy denies, so the plugin that
    # requires it does not load.
    host = stanchion.Stanchion()
    report = host.load(caller_root)

    assert report.loaded == []
    assert len(report.failures) == 1


def test_a_policy_can_narrow_what_a_plugin_asked_for(tmp_path: Path) -> None:
    from conftest import CALLER, write_plugin

    write_plugin(
        tmp_path,
        "caller",
        'name = "caller"\nentry = "init.lua"\n\n[capabilities.kv]\nkeys = ["*"]\n',
        CALLER,
    )

    seen: list[stanchion.CapabilityCall] = []

    def policy(request: stanchion.CapabilityRequest) -> stanchion.Decision:
        assert request.capability == "kv"
        assert request.params == {"keys": ["*"]}
        assert request.signer == "unsigned"
        return stanchion.Decision.grant_with({"keys": ["alpha"]})

    def kv(call: stanchion.CapabilityCall) -> str:
        seen.append(call)
        return "ok"

    host = stanchion.Stanchion(capabilities={"kv": kv}, policy=policy)
    host.load(tmp_path)
    host.call("caller", "lookup", "alpha")

    # The provider must see what policy approved, not what the manifest asked for.
    assert seen[0].grant == {"keys": ["alpha"]}


def test_a_denying_policy_keeps_the_plugin_out(caller_root: Path) -> None:
    def policy(_request: stanchion.CapabilityRequest) -> stanchion.Decision:
        return stanchion.Decision.deny("not in this deployment")

    host = stanchion.Stanchion(capabilities={"kv": lambda _c: None}, policy=policy)
    report = host.load(caller_root)

    assert report.loaded == []
    assert "not in this deployment" in report.failures[0].reason


def test_a_policy_that_raises_denies_rather_than_crashing(caller_root: Path) -> None:
    def policy(_request: stanchion.CapabilityRequest) -> stanchion.Decision:
        raise RuntimeError("the policy is broken")

    host = stanchion.Stanchion(capabilities={"kv": lambda _c: None}, policy=policy)
    report = host.load(caller_root)

    assert report.loaded == []
    assert "the policy is broken" in report.failures[0].reason


def test_a_policy_returning_the_wrong_type_denies(caller_root: Path) -> None:
    host = stanchion.Stanchion(
        capabilities={"kv": lambda _c: None},
        policy=lambda _request: "yes please",  # type: ignore[arg-type,return-value]
    )
    report = host.load(caller_root)

    assert report.loaded == []
    assert "must return a Decision" in report.failures[0].reason


def test_revoking_defangs_a_live_plugin(caller_root: Path) -> None:
    host = stanchion.Stanchion(capabilities={"kv": lambda _c: "ok"})
    host.load(caller_root)

    assert host.call("caller", "lookup", "k") == "ok"
    assert host.revoke("caller", "kv") is True
    with pytest.raises(stanchion.LuaError):
        host.call("caller", "lookup", "k")
    assert host.revoke("caller", "kv") is False


def test_a_provider_that_re_enters_gets_an_error_rather_than_a_hang(
    caller_root: Path,
) -> None:
    """The deadlock this must not be.

    A provider runs while the registry is locked. Calling back in would hang the
    process with no error and no stack, so it raises instead.
    """
    captured: list[BaseException] = []

    def kv(_call: stanchion.CapabilityCall) -> str:
        try:
            host.call("caller", "lookup", "again")
        except BaseException as err:  # noqa: BLE001 - recording it is the point
            captured.append(err)
            raise
        return "unreachable"

    host = stanchion.Stanchion(capabilities={"kv": kv})
    host.load(caller_root)

    with pytest.raises(stanchion.LuaError):
        host.call("caller", "lookup", "k")

    assert len(captured) == 1
    assert isinstance(captured[0], stanchion.ReentrantError)


def test_two_registries_may_nest(caller_root: Path, echo_root: Path) -> None:
    # Different locks, so there is no hazard and this stays allowed.
    inner = stanchion.Stanchion()
    inner.load(echo_root)

    outer = stanchion.Stanchion(
        capabilities={"kv": lambda _call: inner.call("echo", "tagged")}
    )
    outer.load(caller_root)

    assert outer.call("caller", "lookup", "k") == "first"

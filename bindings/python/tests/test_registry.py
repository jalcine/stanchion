"""The registry surface, driven the way a Python application drives it."""

from __future__ import annotations

from pathlib import Path
from typing import Any

import pytest

import stanchion
from conftest import ECHO, write_plugin


def test_a_plugin_loads_and_answers(echo_root: Path) -> None:
    host = stanchion.Stanchion()
    report = host.load(echo_root)

    assert report.is_clean
    assert report.loaded == ["echo"]
    assert host.names() == ["echo"]
    assert len(host) == 1
    assert host.call("echo", "tagged") == "first"
    assert host.call("echo", "sum", 2, 3) == 5


@pytest.mark.parametrize(
    "value",
    [None, True, False, 7, -9, 1.5, "hello", [1, 2, 3], {"k": False}, [], {"a": [1, {"b": 2}]}],
)
def test_values_survive_the_boundary_in_both_directions(
    echo_root: Path, value: Any
) -> None:
    host = stanchion.Stanchion()
    host.load(echo_root)
    assert host.call("echo", "identity", value) == value


def test_a_tuple_arrives_as_a_list(echo_root: Path) -> None:
    host = stanchion.Stanchion()
    host.load(echo_root)
    # Lua has no tuple, so one crosses as a sequence and comes back a list.
    assert host.call("echo", "identity", (1, 2)) == [1, 2]


def test_booleans_do_not_collapse_into_integers(echo_root: Path) -> None:
    host = stanchion.Stanchion()
    host.load(echo_root)
    # `bool` subclasses `int` in Python; a naive conversion turns True into 1.
    assert host.call("echo", "identity", True) is True


def test_a_table_comes_back_as_a_dict(echo_root: Path) -> None:
    host = stanchion.Stanchion()
    host.load(echo_root)
    assert host.call("echo", "shape") == {"n": 1, "xs": ["a", "b"]}


def test_an_unknown_plugin_raises_its_own_error(echo_root: Path) -> None:
    host = stanchion.Stanchion()
    host.load(echo_root)
    with pytest.raises(stanchion.UnknownPluginError):
        host.call("absent", "tagged")


def test_a_raising_method_becomes_a_lua_error(echo_root: Path) -> None:
    host = stanchion.Stanchion()
    host.load(echo_root)
    with pytest.raises(stanchion.LuaError, match="deliberate"):
        host.call("echo", "boom")


def test_an_unconvertible_argument_is_refused(echo_root: Path) -> None:
    host = stanchion.Stanchion()
    host.load(echo_root)
    with pytest.raises(TypeError, match="cannot pass"):
        host.call("echo", "identity", object())  # type: ignore[arg-type]


def test_one_bad_plugin_does_not_stop_the_others(tmp_path: Path) -> None:
    write_plugin(tmp_path, "good", 'name = "good"\nentry = "init.lua"\n', ECHO)
    write_plugin(tmp_path, "bad", 'name = "bad"\nentry = "init.lua"\n', "not lua (((")

    host = stanchion.Stanchion()
    report = host.load(tmp_path)

    assert report.loaded == ["good"]
    assert not report.is_clean
    assert [failure.plugin for failure in report.failures] == ["bad"]


def test_a_dispatch_collects_every_plugin(tmp_path: Path) -> None:
    for name, tag in (("a", "alpha"), ("b", "beta")):
        write_plugin(
            tmp_path,
            name,
            f'name = "{name}"\nentry = "init.lua"\n\n[config]\ntag = "{tag}"\n',
            ECHO,
        )

    host = stanchion.Stanchion()
    host.load(tmp_path)

    tags = [o.value for o in host.dispatch("tagged") if o.value is not None]
    assert sorted(tags) == ["alpha", "beta"]
    # A failing method is reported in place rather than raised.
    assert all(o.error is not None for o in host.dispatch("boom"))


def test_a_reload_picks_up_new_source(tmp_path: Path) -> None:
    write_plugin(
        tmp_path,
        "echo",
        'name = "echo"\nentry = "init.lua"\n\n[config]\ntag = "before"\n',
        ECHO,
    )
    host = stanchion.Stanchion()
    host.load(tmp_path)
    assert host.call("echo", "tagged") == "before"

    (tmp_path / "echo" / "plugin.toml").write_text(
        'name = "echo"\nentry = "init.lua"\n\n[config]\ntag = "after"\n'
    )
    host.reload("echo")
    assert host.call("echo", "tagged") == "after"


def test_a_configured_root_is_used_when_none_is_passed(echo_root: Path) -> None:
    host = stanchion.Stanchion(plugins=echo_root)
    assert host.load().is_clean
    assert host.names() == ["echo"]


def test_loading_without_any_root_says_so() -> None:
    host = stanchion.Stanchion()
    with pytest.raises(stanchion.ConfigError):
        host.load()


def test_an_unknown_standard_library_is_a_configuration_error() -> None:
    with pytest.raises(stanchion.ConfigError, match="sorcery"):
        stanchion.Stanchion(libs=["sorcery"])


def test_isolation_is_per_plugin_unless_asked_otherwise() -> None:
    assert stanchion.Stanchion().isolation == "per-plugin"
    assert stanchion.Stanchion(shared=True).isolation == "shared"


def test_a_listing_reports_what_was_granted(echo_root: Path) -> None:
    host = stanchion.Stanchion()
    host.load(echo_root)

    (info,) = host.list()
    assert info.name == "echo"
    assert info.signer == "unsigned"
    assert info.granted == []


def test_an_audit_does_not_run_the_plugin(tmp_path: Path) -> None:
    write_plugin(
        tmp_path,
        "caller",
        'name = "caller"\nentry = "init.lua"\n\n[capabilities.kv]\n',
        "error('this plugin must never be executed')",
    )
    host = stanchion.Stanchion()
    (entry,) = host.audit(tmp_path)

    assert entry.plugin == "caller"
    assert entry.capabilities == ["kv"]

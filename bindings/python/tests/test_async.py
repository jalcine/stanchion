"""The async surface: plugin methods that yield, awaited from asyncio."""

from __future__ import annotations

import asyncio
from pathlib import Path

import pytest

import stanchion


async def test_an_async_call_drives_a_yielding_method(yielding_root: Path) -> None:
    host = stanchion.Stanchion()
    assert host.load(yielding_root).is_clean

    assert await host.call_async("sleeper", "tagged") == "awake"
    assert await host.call_async("sleeper", "double", 21) == 42


async def test_an_async_dispatch_collects_every_plugin(tmp_path: Path) -> None:
    from conftest import YIELDING, write_plugin

    for name, tag in (("a", "alpha"), ("b", "beta")):
        write_plugin(
            tmp_path,
            name,
            f'name = "{name}"\nentry = "init.lua"\n\n[config]\ntag = "{tag}"\n',
            YIELDING,
        )

    host = stanchion.Stanchion()
    host.load(tmp_path)

    outcomes = await host.dispatch_async("tagged")
    tags = [o.value for o in outcomes if o.value is not None]
    assert sorted(tags) == ["alpha", "beta"]

    failed = await host.dispatch_async("boom")
    assert all(o.error is not None for o in failed)


async def test_an_unknown_plugin_raises_on_the_async_path_too() -> None:
    host = stanchion.Stanchion()
    with pytest.raises(stanchion.UnknownPluginError):
        await host.call_async("absent", "tagged")


async def test_the_event_loop_keeps_running_during_a_call(yielding_root: Path) -> None:
    """The GIL goes back while Lua runs, so other tasks make progress."""
    host = stanchion.Stanchion()
    host.load(yielding_root)

    ticks = 0

    async def tick() -> None:
        nonlocal ticks
        for _ in range(3):
            await asyncio.sleep(0)
            ticks += 1

    ticker = asyncio.create_task(tick())
    assert await host.call_async("sleeper", "tagged") == "awake"
    await ticker

    assert ticks == 3

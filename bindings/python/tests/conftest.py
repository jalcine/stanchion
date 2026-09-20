from __future__ import annotations

from pathlib import Path

import pytest

ECHO = """
local P = {}
P.__index = P

function P.new(config)
  return setmetatable({ tag = config.tag or "echo" }, P)
end

function P:identity(value) return value end
function P:tagged() return self.tag end
function P:sum(a, b) return a + b end
function P:shape() return { n = 1, xs = {"a", "b"} } end
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
function P.new(config) return setmetatable({ tag = config.tag or "y" }, P) end
function P:tagged() coroutine.yield() return self.tag end
function P:double(n) coroutine.yield() return n * 2 end
function P:boom() coroutine.yield() error("deliberate") end
return P
"""


def write_plugin(root: Path, name: str, manifest: str, source: str) -> None:
    directory = root / name
    directory.mkdir(parents=True, exist_ok=True)
    (directory / "plugin.toml").write_text(manifest)
    (directory / "init.lua").write_text(source)


@pytest.fixture
def echo_root(tmp_path: Path) -> Path:
    write_plugin(
        tmp_path,
        "echo",
        'name = "echo"\nentry = "init.lua"\n\n[config]\ntag = "first"\n',
        ECHO,
    )
    return tmp_path


@pytest.fixture
def caller_root(tmp_path: Path) -> Path:
    write_plugin(
        tmp_path,
        "caller",
        'name = "caller"\nentry = "init.lua"\n\n[capabilities.kv]\n',
        CALLER,
    )
    return tmp_path


@pytest.fixture
def yielding_root(tmp_path: Path) -> Path:
    write_plugin(
        tmp_path,
        "sleeper",
        'name = "sleeper"\nentry = "init.lua"\n\n[config]\ntag = "awake"\n',
        YIELDING,
    )
    return tmp_path

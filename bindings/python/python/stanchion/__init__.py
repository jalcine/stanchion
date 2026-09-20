"""Lua plugins for Python applications.

Plugins live in ``<root>/<plugin>/plugin.toml`` alongside their entry chunk, and are
called by name::

    import stanchion

    host = stanchion.Stanchion(allow=["log"])
    report = host.load("plugins/")
    if not report.is_clean:
        for failure in report.failures:
            print(failure.plugin, failure.reason)

    print(host.call("greeter", "greet", "world"))

Python code can answer capabilities the plugins declare. A provider is any callable
taking one :class:`CapabilityCall`::

    def kv(call: stanchion.CapabilityCall) -> object:
        return store[call.args[0]]

    host = stanchion.Stanchion(capabilities={"kv": kv})

Do not call back into the registry from a provider: it runs while that registry is
locked, and the attempt raises :class:`ReentrantError` rather than deadlocking.

See https://github.com/jalcine/stanchion/blob/main/docs/bindings.md.
"""

from __future__ import annotations

from ._stanchion import (
    AuditEntry,
    CapabilityCall,
    CapabilityError,
    CapabilityRequest,
    ConfigError,
    Decision,
    Failure,
    LoadReport,
    LuaError,
    Outcome,
    PluginError,
    PluginInfo,
    ReentrantError,
    Stanchion,
    StanchionError,
    UnknownPluginError,
)

__all__ = [
    "AuditEntry",
    "CapabilityCall",
    "CapabilityError",
    "CapabilityRequest",
    "ConfigError",
    "Decision",
    "Failure",
    "LoadReport",
    "LuaError",
    "Outcome",
    "PluginError",
    "PluginInfo",
    "ReentrantError",
    "Stanchion",
    "StanchionError",
    "UnknownPluginError",
]

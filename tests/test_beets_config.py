"""Beets config/plugin loading behaviour."""

from pathlib import Path
from unittest.mock import MagicMock

from muzik.core.beets import config as cfg_mod


def test_open_library_loads_config_and_plugins_once_per_path(monkeypatch) -> None:
    # Reloading clears the config and drops plugin-registered defaults (they are
    # not re-added), so config+plugins must load once per path, not every call.
    monkeypatch.setattr(cfg_mod, "_LOADED_CONFIG_KEY", None)
    loads: list = []
    plugin_loads: list = []
    monkeypatch.setattr(cfg_mod, "load_config", lambda p=None: loads.append(p))
    monkeypatch.setattr(
        cfg_mod.plugins, "load_plugins", lambda *a, **k: plugin_loads.append(1)
    )
    monkeypatch.setattr(cfg_mod.plugins, "send", lambda *a, **k: None)
    monkeypatch.setattr(cfg_mod, "Library", lambda *a, **k: "lib")
    monkeypatch.setattr(cfg_mod, "config", MagicMock())

    first = Path("/tmp/one.yaml")
    cfg_mod.open_library(first)
    cfg_mod.open_library(first)  # same path -> no reload
    cfg_mod.open_library(Path("/tmp/two.yaml"))  # new path -> reload

    assert loads == [first, Path("/tmp/two.yaml")]
    assert len(plugin_loads) == 2

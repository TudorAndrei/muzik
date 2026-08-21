"""Beets configuration and library loading."""

from __future__ import annotations

from pathlib import Path

from beets import config, plugins
from beets.library import Library


def load_config(config_path: Path | None = None) -> None:
    """Reset and load the Beets configuration for one import session.

    Beets configuration is process-global. Loading it before applying muzik's
    runtime options ensures a selected YAML file cannot override CLI choices.
    """
    config.clear()
    config.read(user=config_path is None)
    if config_path is not None:
        config.set_file(str(config_path))


# Track the config already loaded in this process. Reloading clears the config,
# which drops the default options plugins register in their __init__ (they are
# not re-added on a second load_plugins), so a second open_library would crash
# on e.g. musicbrainz.search_query_ascii. Load once per config path instead.
_LOADED_CONFIG_KEY: str | None = None


def open_library(config_path: Path | None = None) -> Library:
    """Load beets config/plugins (once per process) and open the library."""
    global _LOADED_CONFIG_KEY
    key = str(config_path) if config_path is not None else "__user__"
    if _LOADED_CONFIG_KEY != key:
        load_config(config_path)
        plugins.load_plugins()
        _LOADED_CONFIG_KEY = key

    lib = Library(
        config["library"].as_filename(),
        config["directory"].as_filename(),
    )
    plugins.send("library_opened", lib=lib)
    return lib

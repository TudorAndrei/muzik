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


def open_library(config_path: Path | None = None) -> Library:
    """Load beets config/plugins and open the configured library."""
    load_config(config_path)
    plugins.load_plugins()

    lib = Library(
        config["library"].as_filename(),
        config["directory"].as_filename(),
    )
    plugins.send("library_opened", lib=lib)
    return lib

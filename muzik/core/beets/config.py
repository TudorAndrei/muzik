"""Beets configuration and library loading."""

from __future__ import annotations

from pathlib import Path

from beets import config, plugins
from beets.library import Library


def open_library(config_path: Path | None = None) -> Library:
    """Load beets config/plugins and open the configured library."""
    if config_path is not None:
        config.set_file(str(config_path))
    plugins.load_plugins()

    lib = Library(
        config["library"].as_filename(),
        config["directory"].as_filename(),
    )
    plugins.send("library_opened", lib=lib)
    return lib

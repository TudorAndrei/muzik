"""Shared packaged branding assets."""

from __future__ import annotations

from importlib import resources
from pathlib import Path


LOGO_NAME = "muzik-logo-v2.png"


def logo_path() -> Path | None:
    """Return the packaged or source-tree logo path."""
    try:
        packaged = resources.files("muzik").joinpath(f"assets/{LOGO_NAME}")
        if packaged.is_file():
            return Path(str(packaged))
    except ModuleNotFoundError, FileNotFoundError, TypeError:
        pass
    repo = Path(__file__).resolve().parents[1] / "assets" / LOGO_NAME
    return repo if repo.is_file() else None

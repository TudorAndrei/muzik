"""Start the GPUI desktop application from the active Python installation."""

from __future__ import annotations

import os
from pathlib import Path
import sys

import typer


_BINARY = "muzik-gpui.exe" if sys.platform == "win32" else "muzik-gpui"


def gui_binary() -> Path:
    """Find the installed binary or a binary built in this source checkout."""
    override = os.environ.get("MUZIK_GPUI_BIN")
    if override:
        candidates = (Path(override).expanduser(),)
    else:
        package = Path(__file__).resolve().parents[1]
        repo = package.parent
        candidates = (
            package / "bin" / _BINARY,
            repo / "rust" / "gpui_app" / "target" / "release" / _BINARY,
            repo / "rust" / "gpui_app" / "target" / "debug" / _BINARY,
        )
    for candidate in candidates:
        if candidate.is_file() and os.access(candidate, os.X_OK):
            return candidate
    raise FileNotFoundError(
        "GPUI desktop binary is missing. Build it with "
        "`cargo build --manifest-path rust/gpui_app/Cargo.toml --release` "
        "or set MUZIK_GPUI_BIN to its path."
    )


def gui_cmd() -> None:
    """Open the GPUI Kit desktop interface."""
    try:
        binary = gui_binary()
    except FileNotFoundError as exc:
        typer.echo(str(exc), err=True)
        raise typer.Exit(1) from exc
    environment = os.environ.copy()
    environment["MUZIK_PYTHON"] = sys.executable
    os.execve(binary, [str(binary)], environment)

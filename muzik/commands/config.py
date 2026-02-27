"""music config [show|set-library|edit] — manage beets config."""

import os
import subprocess
import shutil
from pathlib import Path
from typing import Optional

import typer
import yaml
from rich.syntax import Syntax
from rich.table import Table

from muzik.config import BEETS_CONFIG
from muzik.ui.console import console

app = typer.Typer(help="Manage beets configuration.")


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _load_config(path: Path) -> dict:
    if not path.exists():
        return {}
    return yaml.safe_load(path.read_text(encoding="utf-8")) or {}


def _save_config(path: Path, data: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        yaml.dump(data, default_flow_style=False, allow_unicode=True),
        encoding="utf-8",
    )


def _find_editor() -> str:
    for var in ("EDITOR", "VISUAL"):
        editor = os.environ.get(var, "").strip()
        if editor and shutil.which(editor.split()[0]):
            return editor
    for fallback in ("nano", "vi", "vim"):
        if shutil.which(fallback):
            return fallback
    return "vi"


# ---------------------------------------------------------------------------
# Commands
# ---------------------------------------------------------------------------


@app.command("show")
def config_show(
    config: Optional[Path] = typer.Option(
        None,
        "--config",
        "-c",
        help=f"Beets config file to read (default: {BEETS_CONFIG}).",
    ),
) -> None:
    """Show current beets config path and key settings."""
    cfg_path = config or BEETS_CONFIG

    table = Table(show_header=False, border_style="dim", box=None)
    table.add_column("Key", style="bold cyan", width=18)
    table.add_column("Value")

    table.add_row("Config file", str(cfg_path))
    table.add_row(
        "Exists", "[green]yes[/green]" if cfg_path.exists() else "[red]no[/red]"
    )

    data = _load_config(cfg_path)
    table.add_row("library dir", str(data.get("directory", "[dim]not set[/dim]")))
    table.add_row("library db", str(data.get("library", "[dim]not set[/dim]")))
    table.add_row(
        "plugins",
        ", ".join(data.get("plugins", []))
        if data.get("plugins")
        else "[dim]none[/dim]",
    )

    console.print(table)

    if data:
        console.print()
        raw = yaml.dump(data, default_flow_style=False, allow_unicode=True)
        console.print(Syntax(raw, "yaml", theme="ansi_dark", line_numbers=False))


@app.command("set-library")
def config_set_library(
    directory: Path = typer.Argument(..., help="Path to your music library folder."),
    db: Optional[Path] = typer.Option(
        None,
        "--db",
        help="Path for the beets SQLite database (default: <directory>/.library.db).",
    ),
    config: Optional[Path] = typer.Option(
        None,
        "--config",
        "-c",
        help=f"Beets config file to update (default: {BEETS_CONFIG}).",
    ),
) -> None:
    """Set the beets music library directory (and optionally the DB path).

    Creates the config file if it doesn't exist yet.
    """
    cfg_path = config or BEETS_CONFIG
    lib_dir = directory.expanduser().resolve()
    db_path = db.expanduser().resolve() if db else lib_dir / ".library.db"

    data = _load_config(cfg_path)

    old_dir = data.get("directory")
    data["directory"] = str(lib_dir)
    data["library"] = str(db_path)

    # Ensure sensible defaults are present for a first-time config
    if "paths" not in data:
        data["paths"] = {
            "default": "$albumartist/$album%aunique{}/$track $title",
            "singleton": "Non-Album/$artist/$title",
            "comp": "Compilations/$album%aunique{}/$track $title",
        }
    if "import" not in data:
        data["import"] = {
            "write": True,
            "copy": True,
        }

    _save_config(cfg_path, data)

    # Create the library dir if needed
    lib_dir.mkdir(parents=True, exist_ok=True)

    console.print(f"[bold]Beets config:[/bold] {cfg_path}")
    if old_dir and old_dir != str(lib_dir):
        console.print(f"  [dim]directory:[/dim] {old_dir} → [green]{lib_dir}[/green]")
    else:
        console.print(f"  [dim]directory:[/dim] [green]{lib_dir}[/green]")
    console.print(f"  [dim]library db:[/dim] [green]{db_path}[/green]")
    console.print(
        "\n[green]Config saved.[/green]  Run [bold]music config show[/bold] to verify."
    )


@app.command("edit")
def config_edit(
    config: Optional[Path] = typer.Option(
        None,
        "--config",
        "-c",
        help=f"Beets config file to edit (default: {BEETS_CONFIG}).",
    ),
) -> None:
    """Open the beets config file in $EDITOR (creates it if needed)."""
    cfg_path = config or BEETS_CONFIG

    if not cfg_path.exists():
        cfg_path.parent.mkdir(parents=True, exist_ok=True)
        cfg_path.write_text(
            "# beets configuration\n"
            "# See https://beets.readthedocs.io/en/stable/reference/config.html\n\n"
            "directory: ~/music\n"
            "library: ~/music/.library.db\n",
            encoding="utf-8",
        )
        console.print(f"[dim]Created {cfg_path}[/dim]")

    editor = _find_editor()
    console.print(f"[dim]Opening {cfg_path} in {editor!r}…[/dim]")
    subprocess.run([*editor.split(), str(cfg_path)])

"""muzik init — create XDG directories and configure beets for use with muzik."""

import re
from pathlib import Path

import typer

from muzik.config import BEETS_CONFIG, CACHE_DIR, DEFAULT_DOWNLOAD_DIR, DEFAULT_SPLITS_DIR
from muzik.ui.console import console


# The beets import setting muzik requires.
_IMPORT_BLOCK = "import:\n  duplicate_action: skip\n"
_DUPLICATE_LINE = "  duplicate_action: skip"


def _ensure_dirs() -> None:
    dirs = {
        "Downloads": DEFAULT_DOWNLOAD_DIR,
        "Splits   ": DEFAULT_SPLITS_DIR,
        "Cache    ": CACHE_DIR,
        "Beets cfg": BEETS_CONFIG.parent,
    }
    for label, d in dirs.items():
        existed = d.exists()
        d.mkdir(parents=True, exist_ok=True)
        status = "[dim]already exists[/dim]" if existed else "[green]created[/green]"
        console.print(f"  {label}  {d}  {status}")


def _configure_beets() -> None:
    """Ensure BEETS_CONFIG contains ``import.duplicate_action: skip``.

    • If the config file doesn't exist yet, a minimal one is created.
    • If it already has ``duplicate_action`` set, nothing is changed.
    • If it has an ``import:`` section but no ``duplicate_action``, the line
      is inserted right after ``import:``.
    • Otherwise the full ``import:`` block is appended.
    """
    cfg = BEETS_CONFIG

    if not cfg.exists():
        cfg.write_text(
            "# beets config — created by muzik init\n"
            "# See https://beets.readthedocs.io/en/stable/reference/config.html\n\n"
            + _IMPORT_BLOCK
        )
        console.print(f"  Beets cfg  {cfg}  [green]created[/green]")
        return

    text = cfg.read_text()

    if "duplicate_action" in text:
        console.print(
            f"  Beets cfg  {cfg}  [dim]duplicate_action already set — skipped[/dim]"
        )
        return

    # Insert after an existing `import:` line if present
    if re.search(r"^import\s*:", text, re.MULTILINE):
        text = re.sub(
            r"(^import\s*:[ \t]*$)",
            f"\\1\n{_DUPLICATE_LINE}",
            text,
            count=1,
            flags=re.MULTILINE,
        )
    else:
        text = text.rstrip("\n") + "\n\n" + _IMPORT_BLOCK

    cfg.write_text(text)
    console.print(f"  Beets cfg  {cfg}  [green]added duplicate_action: skip[/green]")


def init_cmd() -> None:
    """Set up muzik: create XDG directories and configure beets.

    \b
    Creates:
      $XDG_DATA_HOME/muzik/downloads   — default download directory
      $XDG_DATA_HOME/muzik/splits      — default splits directory
      $XDG_CACHE_HOME/music-scripts    — cache directory
      $XDG_CONFIG_HOME/beets/          — beets config directory

    \b
    Beets config changes:
      Sets import.duplicate_action = skip so that albums already present in
      the library are silently skipped on every workflow re-run.
      Existing settings are preserved; the file is only written if the
      setting is missing.
    """
    console.print("[bold]Directories[/bold]")
    _ensure_dirs()

    console.print("\n[bold]Beets configuration[/bold]")
    _configure_beets()

    console.rule()
    console.print("[bold green]muzik init complete.[/bold green]")
    console.print(
        "\n[dim]Run [bold]muzik workflow <url>[/bold] to start downloading.[/dim]"
    )

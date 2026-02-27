"""muzik import — import an existing music library into beets."""

import sys
from pathlib import Path
from typing import Optional

import typer

from muzik.config import BEETS_CONFIG
from muzik.core.runner import run_passthrough
from muzik.ui.console import console, err


def _beet_bin() -> str:
    beet = Path(sys.executable).parent / "beet"
    return str(beet) if beet.exists() else "beet"


def import_cmd(
    directory: Path = typer.Argument(..., help="Root directory of the existing music library to import."),
    copy: bool = typer.Option(
        False,
        "--copy",
        "-C",
        help="Copy files into the beets library directory (default: move).",
    ),
    link: bool = typer.Option(
        False,
        "--link",
        "-L",
        help="Symlink files instead of moving or copying.",
    ),
    nowrite: bool = typer.Option(
        False,
        "--nowrite",
        help="Do not write tags to files when importing.",
    ),
    quiet: bool = typer.Option(
        False,
        "--quiet",
        "-q",
        help="Quiet mode — skip albums that require user input (non-interactive).",
    ),
    dry_run: bool = typer.Option(
        False,
        "--dry-run",
        "-d",
        help="Show what beets would do without making changes.",
    ),
    config: Optional[Path] = typer.Option(
        None,
        "--config",
        "-c",
        help=f"Beets config file (default: {BEETS_CONFIG}).",
    ),
) -> None:
    """Import an existing music library into beets.

    Runs ``beet import`` with ``--incremental`` so already-imported albums are
    skipped.  By default files are **moved** into the beets library directory.
    Use ``--copy`` to keep originals in place, or ``--link`` to create symlinks.

    Run ``muzik init`` first to make sure beets is configured.
    """
    beets_cfg = config or BEETS_CONFIG

    if not directory.exists():
        err(f"[red]Directory not found: {directory}[/red]")
        raise typer.Exit(1)

    if not beets_cfg.exists():
        err(
            f"[yellow]Beets config not found at {beets_cfg}.[/yellow] "
            "Run [bold]muzik init[/bold] to create one."
        )

    cmd = [_beet_bin()]
    if beets_cfg.exists():
        cmd += ["-c", str(beets_cfg)]

    subcmd = ["import", "--incremental"]

    if dry_run:
        subcmd.append("--pretend")
    if copy:
        subcmd.append("--copy")
    elif link:
        subcmd.append("--link")
    else:
        subcmd.append("--move")
    if nowrite:
        subcmd.append("--nowrite")
    if quiet:
        subcmd.append("--quiet")

    cmd += subcmd + [str(directory)]

    console.print(f"[bold]beet import[/bold] {directory}")
    rc = run_passthrough(cmd)
    if rc != 0:
        err(f"[red]beet exited with code {rc}[/red]")
        raise typer.Exit(rc)

    console.print("[green]Import complete.[/green]")

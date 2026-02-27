"""music organize <dir> — beets passthrough for tagging and importing."""

import sys
from pathlib import Path
from typing import Optional

import typer

from muzik.config import BEETS_CONFIG
from muzik.core.runner import run_passthrough
from muzik.ui.console import console, err


def _beet_bin() -> str:
    """Return the path to the beet binary in the same venv as this Python."""
    beet = Path(sys.executable).parent / "beet"
    return str(beet) if beet.exists() else "beet"


def organize_cmd(
    directory: Path = typer.Argument(..., help="Directory containing audio tracks."),
    import_: bool = typer.Option(
        False,
        "--import",
        "-i",
        help="Import files into beets library (moves files).",
    ),
    tag_only: bool = typer.Option(
        False,
        "--tag-only",
        "-t",
        help="Only write tags; do not move or import files.",
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
    """Organize/tag audio files using beets.

    Passes stdin/stdout directly to beet so interactive prompts work.
    Run ``muzik init`` first to configure beets with sensible defaults
    (duplicate_action: skip).
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
        # Don't abort — beet itself will handle the missing config

    # Build beet command — use venv-local binary so it works under `uv run`
    cmd = [_beet_bin()]
    if beets_cfg.exists():
        cmd += ["-c", str(beets_cfg)]

    if import_:
        console.print(f"[bold]beet import[/bold] {directory}")
        subcmd = ["import", "--incremental"]
        if dry_run:
            subcmd.append("--pretend")
        cmd += subcmd + [str(directory)]
    elif tag_only:
        console.print(f"[bold]beet write[/bold] (tag-only) {directory}")
        cmd += ["write"] + (["--yes"] if not dry_run else []) + [str(directory)]
    else:
        # Default: import with auto-tagging, skipping already-imported directories
        console.print(f"[bold]beet import[/bold] {directory}")
        subcmd = ["import", "--incremental"]
        if dry_run:
            subcmd.append("--pretend")
        cmd += subcmd + [str(directory)]

    rc = run_passthrough(cmd)
    if rc != 0:
        err(f"[red]beet exited with code {rc}[/red]")
        raise typer.Exit(rc)

    console.print("[green]beet finished.[/green]")

"""music organize <dir> — beets tagging and importing."""

import sys
from pathlib import Path
from typing import Optional

import typer

from muzik.config import BEETS_CONFIG
from muzik.core.beets.decisions import NonInteractiveBeetsDecisions
from muzik.core.beets.importer import ImportOptions, import_paths
from muzik.core.runner import run_passthrough
from muzik.ui.console import console, err


def _beet_bin() -> str:
    """Return the path to the beet binary in the same venv as this Python."""
    beet = Path(sys.executable).parent / "beet"
    return str(beet) if beet.exists() else "beet"


def _beet_command(
    directory: Path,
    *,
    tag_only: bool,
    dry_run: bool,
    config: Optional[Path],
) -> list[str]:
    cmd = [_beet_bin()]
    if config and config.exists():
        cmd += ["-c", str(config)]

    if tag_only:
        subcmd = ["write"]
        if not dry_run:
            subcmd.append("--yes")
        return cmd + subcmd + [str(directory)]

    subcmd = ["import", "--incremental", "--move"]
    if dry_run:
        subcmd.append("--pretend")
    return cmd + subcmd + [str(directory)]


def _run_beet_passthrough(
    directory: Path,
    *,
    tag_only: bool,
    dry_run: bool,
    config: Optional[Path],
) -> None:
    if tag_only:
        console.print(f"[bold]beet write[/bold] (tag-only) {directory}")
    else:
        console.print(f"[bold]beet import[/bold] {directory}")

    rc = run_passthrough(
        _beet_command(
            directory,
            tag_only=tag_only,
            dry_run=dry_run,
            config=config,
        )
    )
    if rc != 0:
        err(f"[red]beet exited with code {rc}[/red]")
        raise typer.Exit(rc)


def organize_cmd(
    directory: Path = typer.Argument(..., help="Directory containing audio tracks."),
    import_: bool = typer.Option(
        False,
        "--import",
        "-i",
        help="Import files into beets library by moving them (same as default behavior).",
    ),
    tag_only: bool = typer.Option(
        False,
        "--tag-only",
        "-t",
        help="Only write tags; do not import or move files.",
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

    Uses the internal beets API for imports. Tag-only writes still use the
    isolated beets write subprocess path.
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

    if tag_only:
        _run_beet_passthrough(
            directory,
            tag_only=tag_only,
            dry_run=dry_run,
            config=beets_cfg,
        )
        console.print("[green]beet finished.[/green]")
        return

    console.print(f"[bold]beet import[/bold] {directory}")
    try:
        import_paths(
            ImportOptions(
                paths=[directory],
                config_path=beets_cfg if beets_cfg.exists() else None,
                move=True,
                dry_run=dry_run,
                incremental=True,
            ),
            decisions=NonInteractiveBeetsDecisions(),
        )
    except Exception as exc:
        err(f"[red]beets import failed:[/red] {exc}")
        raise typer.Exit(1) from exc

    console.print("[green]beet finished.[/green]")

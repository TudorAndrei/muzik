"""muzik import — import an existing music library into beets."""

import asyncio
from pathlib import Path
from typing import Optional

import typer

from muzik.config import BEETS_CONFIG
from muzik.core.beets.decisions import NonInteractiveBeetsDecisions
from muzik.core.beets.importer import ImportOptions, import_paths
from muzik.ui.console import console, err


def _notify(directory: Path) -> None:
    try:
        from desktop_notifier import DesktopNotifier

        async def _send() -> None:
            notifier = DesktopNotifier(app_name="muzik")
            await notifier.send(
                title="beets needs your input",
                message=f"Importing: {directory.name}",
            )

        asyncio.run(_send())
    except Exception:
        pass


def import_cmd(
    directory: Path = typer.Argument(
        ..., help="Root directory of the existing music library to import."
    ),
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

    Runs a beets import with ``--incremental`` so already-imported albums are
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

    console.print(f"[bold]beet import[/bold] {directory}")
    if not quiet:
        _notify(directory)
    try:
        import_paths(
            ImportOptions(
                paths=[directory],
                config_path=beets_cfg if beets_cfg.exists() else None,
                copy=copy,
                link=link,
                move=not copy and not link,
                nowrite=nowrite,
                quiet=quiet,
                dry_run=dry_run,
                incremental=True,
            ),
            decisions=NonInteractiveBeetsDecisions(quiet=quiet),
        )
    except Exception as exc:
        err(f"[red]beets import failed:[/red] {exc}")
        raise typer.Exit(1) from exc

    console.print("[green]Import complete.[/green]")

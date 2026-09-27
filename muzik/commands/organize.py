"""Organize audio files in the music library."""

from pathlib import Path
from typing import Optional

import typer

from muzik.config import LIBRARY_CONFIG
from muzik.core.import_models import ImportOptions, NonInteractiveImportDecisions
from muzik.core.import_service import organize_paths
from muzik.ui.console import console, err
from muzik.ui.import_events import ConsoleImportEvents


def organize_cmd(
    directory: Path = typer.Argument(..., help="Directory containing audio tracks."),
    import_: bool = typer.Option(
        False,
        "--import",
        "-i",
        help="Import files into the music library by moving them.",
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
        help="Show planned changes without writing them.",
    ),
    config: Optional[Path] = typer.Option(
        None,
        "--config",
        "-c",
        help=f"Library config file (default: {LIBRARY_CONFIG}).",
    ),
) -> None:
    """Import audio or write tags from the music library."""
    config_path = config or LIBRARY_CONFIG
    if not directory.exists():
        err(f"[red]Directory not found: {directory}[/red]")
        raise typer.Exit(1)
    if not config_path.exists():
        err(
            f"[yellow]Library config not found at {config_path}.[/yellow] "
            "Run [bold]muzik init[/bold] to create one."
        )
    action = "Write tags" if tag_only else "Import"
    console.print(f"[bold]{action}[/bold] {directory}")
    try:
        organize_paths(
            ImportOptions(
                paths=[directory],
                config_path=config_path if config_path.exists() else None,
                move=True,
                dry_run=dry_run,
                incremental=True,
            ),
            tag_only=tag_only,
            decisions=NonInteractiveImportDecisions(),
            events=ConsoleImportEvents(),
        )
    except Exception as exc:
        err(f"[red]Organization failed:[/red] {exc}")
        raise typer.Exit(1) from exc
    console.print("[green]Organization complete.[/green]")

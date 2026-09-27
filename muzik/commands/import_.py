"""Import audio into the music library."""

import asyncio
from pathlib import Path
from typing import Optional

import typer

from muzik.config import LIBRARY_CONFIG
from muzik.core.agent_decisions import AgentImportDecisions
from muzik.core.import_models import (
    ImportDecisions,
    ImportOptions,
    NonInteractiveImportDecisions,
)
from muzik.core.import_service import import_paths
from muzik.core.library_prune import PruneAborted, prune_missing_items
from muzik.core.native_import import preview_sync
from muzik.ui.console import console, err
from muzik.ui.import_events import ConsoleImportEvents


def _notify(directory: Path) -> None:
    try:
        from desktop_notifier import DesktopNotifier

        async def _send() -> None:
            notifier = DesktopNotifier(app_name="muzik")
            await notifier.send(
                title="muzik needs your input",
                message=f"Importing: {directory.name}",
            )

        asyncio.run(_send())
    except Exception:
        pass


def import_cmd(
    directory: Optional[Path] = typer.Argument(
        None, help="Root directory of the existing music library to import."
    ),
    agent: bool = typer.Option(
        False,
        "--agent",
        help="Let an LLM auto-pick matches (applies confident ones, skips the rest).",
    ),
    library: Optional[str] = typer.Option(
        None,
        "--library",
        help="Re-tag existing library items matching this query "
        '(instead of importing a directory), e.g. "mb_albumid::^$".',
    ),
    copy: bool = typer.Option(
        False,
        "--copy",
        "-C",
        help="Copy files into the music library directory (default: move).",
    ),
    link: bool = typer.Option(
        False,
        "--link",
        "-L",
        help="Symlink files instead of moving or copying (requires --nowrite).",
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
        help="Show the planned changes without applying them.",
    ),
    no_prune: bool = typer.Option(
        False,
        "--no-prune",
        help="Do not remove library items orphaned by moves after the import.",
    ),
    config: Optional[Path] = typer.Option(
        None,
        "--config",
        "-c",
        help=f"Music library config file (default: {LIBRARY_CONFIG}).",
    ),
) -> None:
    """Import an existing music library.

    Uses incremental history so already-imported albums are skipped.
    By default files are **moved** into the music library directory.
    Use ``--copy`` to keep originals in place, or ``--link`` to create symlinks.

    Pass ``--agent`` to let an LLM pick matches automatically: confident matches
    are applied and uncertain ones are skipped. Pass ``--library`` with a
    query to re-tag items already in the library instead of importing a
    directory (for example ``--library "mb_albumid::^$"`` for unmatched albums).
    Combine with ``--dry-run`` to preview without changing files.

    Run ``muzik init`` first to configure the music library.
    """
    beets_cfg = config or LIBRARY_CONFIG
    library = library if isinstance(library, str) else None
    agent = agent is True

    if directory is None and not library:
        err("[red]Give a DIRECTORY to import, or --library QUERY to re-tag.[/red]")
        raise typer.Exit(1)

    if link and not nowrite:
        err(
            "[red]--link requires --nowrite because linked files cannot be tagged during import.[/red]"
        )
        raise typer.Exit(2)

    if directory is not None and not directory.exists():
        err(f"[red]Directory not found: {directory}[/red]")
        raise typer.Exit(1)

    if not beets_cfg.exists():
        err(
            f"[yellow]Music library config not found at {beets_cfg}.[/yellow] "
            "Run [bold]muzik init[/bold] to create one."
        )

    target = str(directory) if directory is not None else f"library query {library!r}"
    console.print(f"[bold]muzik import[/bold] {target}{' (agent)' if agent else ''}")
    if library and dry_run:
        try:
            albums, items = preview_sync(beets_cfg, library)
        except Exception as exc:
            err(f"[red]Sync preview failed:[/red] {exc}")
            raise typer.Exit(1) from exc
        console.print(f"Sync preview: {albums} albums and {items} items selected.")
        return
    if not quiet and directory is not None:
        _notify(directory)

    decisions: ImportDecisions
    if agent:
        decisions = AgentImportDecisions(
            log=lambda message: console.print(f"[dim]agent:[/dim] {message}")
        )
    else:
        decisions = NonInteractiveImportDecisions(quiet=quiet)

    try:
        import_paths(
            ImportOptions(
                paths=[directory] if directory is not None else [],
                query=library,
                config_path=beets_cfg if beets_cfg.exists() else None,
                copy=copy,
                link=link,
                move=not copy and not link,
                nowrite=nowrite,
                quiet=quiet,
                dry_run=dry_run,
                incremental=True,
            ),
            decisions=decisions,
            events=ConsoleImportEvents(),
        )
    except Exception as exc:
        err(f"[red]Import failed:[/red] {exc}")
        raise typer.Exit(1) from exc

    # A move-mode re-tag can orphan the old entry; prune it so the library
    # stays consistent. Copy/link imports move nothing, so there is nothing
    # to prune.
    moved = not copy and not link
    if moved and not dry_run and not no_prune:
        try:
            pruned = prune_missing_items(beets_cfg if beets_cfg.exists() else None)
            if pruned:
                console.print(f"[dim]Pruned {pruned} item(s) orphaned by moves.[/dim]")
        except PruneAborted as exc:
            err(
                f"[yellow]Skipped auto-prune:[/yellow] {exc}. "
                "Is the library volume mounted? Use --no-prune to silence this."
            )

    console.print("[green]Import complete.[/green]")

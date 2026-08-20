"""muzik import — import an existing music library into beets."""

import asyncio
from pathlib import Path
from typing import Optional

import typer

from muzik.config import BEETS_CONFIG
from muzik.core.beets.agent_decisions import AgentBeetsDecisions
from muzik.core.beets.decisions import BeetsDecisions, NonInteractiveBeetsDecisions
from muzik.core.beets.importer import (
    ImportOptions,
    PruneAborted,
    import_paths,
    prune_missing_items,
)
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
        help="Re-tag existing library items matching this beets query "
        '(instead of importing a directory), e.g. "mb_albumid::^$".',
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
    no_prune: bool = typer.Option(
        False,
        "--no-prune",
        help="Do not remove library items orphaned by moves after the import.",
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

    Pass ``--agent`` to let an LLM pick matches automatically: confident matches
    are applied and uncertain ones are skipped. Pass ``--library`` with a beets
    query to re-tag items already in the library instead of importing a
    directory (for example ``--library "mb_albumid::^$"`` for unmatched albums).
    Combine with ``--dry-run`` to preview without changing files.

    Run ``muzik init`` first to make sure beets is configured.
    """
    beets_cfg = config or BEETS_CONFIG

    if directory is None and not library:
        err("[red]Give a DIRECTORY to import, or --library QUERY to re-tag.[/red]")
        raise typer.Exit(1)

    if directory is not None and not directory.exists():
        err(f"[red]Directory not found: {directory}[/red]")
        raise typer.Exit(1)

    if not beets_cfg.exists():
        err(
            f"[yellow]Beets config not found at {beets_cfg}.[/yellow] "
            "Run [bold]muzik init[/bold] to create one."
        )

    target = str(directory) if directory is not None else f"library query {library!r}"
    console.print(f"[bold]beet import[/bold] {target}{' (agent)' if agent else ''}")
    if not quiet and directory is not None:
        _notify(directory)

    decisions: BeetsDecisions
    if agent:
        decisions = AgentBeetsDecisions(
            log=lambda message: console.print(f"[dim]agent:[/dim] {message}")
        )
    else:
        decisions = NonInteractiveBeetsDecisions(quiet=quiet)

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
        )
    except Exception as exc:
        err(f"[red]beets import failed:[/red] {exc}")
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

"""muzik soulseek — search and download via the embedded Seakarr bridge."""

from __future__ import annotations

import os
from pathlib import Path
from typing import Optional

from beets.library import Item
import typer
from rich.progress import (
    BarColumn,
    Progress,
    SpinnerColumn,
    TextColumn,
    TimeElapsedColumn,
)
from rich.table import Table

from muzik.config import BEETS_CONFIG, DEFAULT_SOULSEEK_DIR
from muzik.commands.organize import organize_cmd
import muzik.core.cache as cache_mod
from muzik.core.beets.config import open_library
from muzik.core.beets.lookup import resolve_item_path
from muzik.core.quality import measure_quality, quality_score
from muzik.core.sources.base import (
    Candidate,
    DownloadRequest,
    QualityInfo,
    ResolvedTrack,
)
from muzik.core.sources.seakarr import (
    SoulseekError,
    SeakarrSource,
    candidate_matches_track,
)
from muzik.core.workflow.decisions import WorkflowDecisionError
from muzik.ui.cli.decisions import CliWorkflowDecisions
from muzik.ui.console import console, err


app = typer.Typer(no_args_is_help=True, help="Search and download from Soulseek.")


def _source() -> SeakarrSource:
    return SeakarrSource()


def _format_size(size: int | None) -> str:
    if size is None:
        return "?"
    value = float(size)
    for unit in ("B", "KB", "MB", "GB"):
        if value < 1024 or unit == "GB":
            return f"{value:.1f}{unit}" if unit != "B" else f"{int(value)}B"
        value /= 1024
    return f"{value:.1f}GB"


def _candidate_id(candidate: Candidate) -> str:
    return cache_mod.candidate_cache_key(candidate).removeprefix(
        f"candidate_{candidate.source}_"
    )


def _candidate_cache_key(candidate_id: str) -> str:
    if candidate_id.startswith("candidate_"):
        return candidate_id
    return f"candidate_soulseek_{candidate_id}"


def _store_candidates(candidates: list[Candidate]) -> None:
    for candidate in candidates:
        cache_mod.set_json(
            _candidate_cache_key(_candidate_id(candidate)),
            candidate.to_dict(),
        )


def _load_candidate(candidate_id: str) -> Candidate:
    data = cache_mod.get_json(_candidate_cache_key(candidate_id))
    if not data:
        raise SoulseekError(f"Candidate not found in cache: {candidate_id}")
    candidate = Candidate.from_dict(data)
    if candidate.source != "soulseek" or not candidate.source_id:
        raise SoulseekError(f"Cached candidate is invalid: {candidate_id}")
    return candidate


def _candidate_table(candidates: list[Candidate], *, limit: int) -> Table:
    table = Table(
        title="Soulseek candidates",
        show_header=True,
        header_style="bold cyan",
        border_style="dim",
    )
    table.add_column("#", justify="right", width=4)
    table.add_column("ID", width=16)
    table.add_column("Score", justify="right", width=7)
    table.add_column("Format", width=8)
    table.add_column("Lossless", width=8)
    table.add_column("Bitrate", justify="right", width=7)
    table.add_column("Size", justify="right", width=9)
    table.add_column("Files", justify="right", width=6)
    table.add_column("User", overflow="fold")
    table.add_column("Path", overflow="fold")

    for idx, candidate in enumerate(candidates[:limit], 1):
        size = sum((file.size or 0) for file in candidate.files) or None
        table.add_row(
            str(idx),
            _candidate_id(candidate),
            f"{candidate.score:.1f}",
            candidate.quality.format or "?",
            "yes" if candidate.quality.lossless else "no",
            str(candidate.quality.bitrate or "?"),
            _format_size(size),
            str(len(candidate.files)),
            candidate.user or "?",
            candidate.path or candidate.title,
        )
    return table


@app.command("check")
def check_cmd() -> None:
    """Verify the embedded Seakarr bridge can connect and log in."""
    try:
        info = _source().check()
    except Exception as exc:
        err(f"[red]Soulseek check failed:[/red] {exc}")
        raise typer.Exit(1) from exc

    console.print(
        "[green]Soulseek reachable[/green]"
        if info["connected"]
        else "[red]Soulseek unreachable[/red]"
    )
    console.print(f"  Username: [dim]{info['username']}[/dim]")
    console.print(f"  Server: [dim]{info['server']}[/dim]")
    console.print(f"  Download dir: [dim]{info['download_dir']}[/dim]")
    console.print(f"  Detail: [dim]{info['detail']}[/dim]")
    if not info["connected"]:
        err(
            "[red]Set MUZIK_SOULSEEK_USERNAME and MUZIK_SOULSEEK_PASSWORD, "
            "then retry.[/red]"
        )
        raise typer.Exit(1)


@app.command("search")
def search_cmd(
    query: str = typer.Argument(..., help="Soulseek search query."),
    prefer: str = typer.Option(
        "lossless",
        "--prefer",
        help="Preferred quality: flac, lossless, mp3-320, or any.",
    ),
    limit: int = typer.Option(20, "--limit", "-n", help="Candidates to display."),
) -> None:
    """Search Soulseek and print ranked candidates."""
    source = _source()
    try:
        resolved = source.resolve(
            DownloadRequest(raw=query, source="soulseek", prefer_format=prefer)
        )
        candidates = source.search(resolved, prefer=prefer, limit=limit)
    except Exception as exc:
        err(f"[red]Soulseek search failed:[/red] {exc}")
        raise typer.Exit(1) from exc

    if not candidates:
        console.print("[yellow]No candidates found.[/yellow]")
        return
    _store_candidates(candidates)
    console.print(_candidate_table(candidates, limit=limit))


@app.command("download")
def download_cmd(
    query: str | None = typer.Argument(None, help="Soulseek search query."),
    prefer: str = typer.Option(
        "lossless",
        "--prefer",
        help="Preferred quality: flac, lossless, mp3-320, or any.",
    ),
    limit: int = typer.Option(10, "--limit", "-n", help="Candidates to consider."),
    output: Path = typer.Option(
        DEFAULT_SOULSEEK_DIR,
        "--output",
        "-o",
        help="Local Soulseek download directory.",
    ),
    no_interactive: bool = typer.Option(
        False,
        "--no-interactive",
        help="Download the highest-ranked candidate without prompting.",
    ),
    no_wait: bool = typer.Option(
        False,
        "--no-wait",
        help="Enqueue downloads without waiting for transfer completion.",
    ),
    no_organize: bool = typer.Option(
        False,
        "--no-organize",
        help="Skip beets organization after downloading.",
    ),
    import_: bool = typer.Option(
        False,
        "--import",
        "-i",
        help="Import to beets library (moves files).",
    ),
    tag_only: bool = typer.Option(
        False,
        "--tag-only",
        "-t",
        help="Only tag files with beets, do not move.",
    ),
    dry_run: bool = typer.Option(
        False,
        "--dry-run",
        "-d",
        help="Show selected candidate without enqueueing downloads.",
    ),
    candidate_id: str | None = typer.Option(
        None,
        "--candidate",
        help="Download a previously cached candidate ID from `muzik soulseek search`.",
    ),
) -> None:
    """Search Soulseek, select a candidate, and enqueue a download."""
    source = _source()
    if candidate_id:
        try:
            candidate = _load_candidate(candidate_id)
        except SoulseekError as exc:
            err(f"[red]Soulseek candidate load failed:[/red] {exc}")
            raise typer.Exit(1) from exc
    else:
        if not query:
            err("[red]Provide a query or --candidate.[/red]")
            raise typer.Exit(1)
        try:
            resolved = source.resolve(
                DownloadRequest(raw=query, source="soulseek", prefer_format=prefer)
            )
            candidates = source.search(resolved, prefer=prefer, limit=limit)
        except Exception as exc:
            err(f"[red]Soulseek search failed:[/red] {exc}")
            raise typer.Exit(1) from exc

        if not candidates:
            console.print("[yellow]No candidates found.[/yellow]")
            raise typer.Exit(0)

        _store_candidates(candidates)
        console.print(_candidate_table(candidates, limit=limit))
        decisions = CliWorkflowDecisions(
            interactive=not no_interactive,
            candidate_limit=limit,
            candidate_prompt="Candidate number",
            display_soulseek_candidates=False,
        )
        try:
            candidate = decisions.choose_soulseek_candidate(candidates)
        except WorkflowDecisionError as exc:
            err(f"[red]{exc}[/red]")
            raise typer.Exit(1) from exc
    if dry_run:
        console.print(f"[dim]Would download:[/dim] {candidate.title}")
        console.print(f"  User: [dim]{candidate.user or '?'}[/dim]")
        console.print(f"  Files: [dim]{len(candidate.files)}[/dim]")
        return

    try:
        result = source.download(candidate, output, wait=not no_wait)
    except SoulseekError as exc:
        err(f"[red]Soulseek download failed:[/red] {exc}")
        raise typer.Exit(1) from exc

    console.print(f"[green]Download enqueued:[/green] {candidate.title}")
    if result.files:
        for file in result.files:
            console.print(f"  [dim]{file}[/dim]")
    else:
        console.print("  [dim]No local files mapped yet.[/dim]")
    if result.metadata_path:
        console.print(f"  Metadata: [dim]{result.metadata_path}[/dim]")

    if not no_organize and result.files:
        target = result.root if len(result.files) > 1 else result.files[0]
        console.print(f"[bold]Organize[/bold] {target}")
        try:
            organize_cmd(
                directory=target,
                import_=import_,
                tag_only=tag_only,
                dry_run=False,
                config=None,
            )
        except (SystemExit, typer.Exit) as exc:
            if getattr(exc, "code", 0) != 0:
                err(f"[red]beet failed for {target}[/red]")
                raise


@app.command("check-library")
def check_library_cmd(
    query: Optional[str] = typer.Option(
        None,
        "--query",
        "-q",
        help="Beets query to scope the scan (e.g. 'albumartist:Etnobotanika'). "
        "Without it, the whole library is scanned.",
    ),
    min_bitrate: int = typer.Option(
        256,
        "--min-bitrate",
        help="Lossy tracks at or above this bitrate (kbps) are left alone.",
    ),
    prefer: str = typer.Option(
        "lossless",
        "--prefer",
        help="Preferred replacement quality: flac, lossless, mp3-320, or any.",
    ),
    limit: int = typer.Option(
        20,
        "--limit",
        "-n",
        help="Maximum number of below-threshold tracks to search Soulseek for.",
    ),
    config: Optional[Path] = typer.Option(
        None,
        "--config",
        "-c",
        help=f"Beets config file (default: {BEETS_CONFIG}).",
    ),
) -> None:
    """Measure real quality across the Beets library and suggest Soulseek replacements.

    Read-only: nothing is downloaded or changed. A printed candidate's ID
    works with `muzik soulseek download --candidate <id>` to fetch it.
    """
    try:
        library = open_library(config)
    except Exception as exc:
        err(f"[red]Could not open the Beets library:[/red] {exc}")
        raise typer.Exit(1) from exc

    directory = os.fsdecode(library.directory)
    scanned = 0
    flagged: list[tuple[Item, QualityInfo, Path]] = []
    for item in library.items(query):
        path = resolve_item_path(directory, item.path)
        if not path.is_file():
            continue
        quality = measure_quality(path)
        if quality is None:
            continue
        scanned += 1
        if quality.lossless:
            continue
        if quality.bitrate is not None and quality.bitrate >= min_bitrate:
            continue
        flagged.append((item, quality, path))

    flagged.sort(
        key=lambda entry: (
            str(entry[0].albumartist or entry[0].artist or ""),
            str(entry[0].album or ""),
            str(entry[0].title or ""),
        )
    )

    if not flagged:
        console.print(
            f"[green]No tracks below {min_bitrate}kbps out of {scanned} scanned.[/green]"
        )
        return

    to_search = flagged[:limit]
    skipped = len(flagged) - len(to_search)

    table = Table(
        title="Library quality check",
        show_header=True,
        header_style="bold cyan",
        border_style="dim",
    )
    table.add_column("Artist", overflow="fold")
    table.add_column("Title", overflow="fold")
    table.add_column("Current", width=14)
    table.add_column("Status", width=18)
    table.add_column("Suggested", width=18)
    table.add_column("Candidate ID", width=16)

    source = _source()
    replacements: list[Candidate] = []
    found = 0
    progress = Progress(
        SpinnerColumn(),
        TextColumn("[progress.description]{task.description}"),
        BarColumn(),
        TextColumn("[progress.percentage]{task.percentage:>3.0f}%"),
        TimeElapsedColumn(),
        console=console,
    )
    with progress:
        task_id = progress.add_task("Checking Soulseek…", total=len(to_search))
        for item, quality, _path in to_search:
            artist = str(item.artist or "?")
            title = str(item.title or "?")
            progress.update(task_id, description=f"{artist} - {title}")
            current = f"{quality.format or '?'} {quality.bitrate or '?'}kbps"
            track = ResolvedTrack(
                title=str(item.title or ""),
                artist=str(item.artist) if item.artist else None,
                album=str(item.album) if item.album else None,
                duration=float(item.length) if item.length else None,
                source="beets",
            )
            try:
                candidates = source.search(track, prefer=prefer, limit=10)
            except Exception as exc:
                table.add_row(
                    artist, title, current, "search failed", str(exc)[:60], ""
                )
                progress.advance(task_id)
                continue

            safe = [c for c in candidates if candidate_matches_track(c, track)]
            current_score = quality_score(quality, prefer)
            better = next(
                (c for c in safe if quality_score(c.quality, prefer) > current_score),
                None,
            )
            if better is None:
                table.add_row(artist, title, current, "no safe match", "", "")
                progress.advance(task_id)
                continue

            found += 1
            replacements.append(better)
            suggested = (
                f"{better.quality.format or '?'} {better.quality.bitrate or '?'}kbps"
            )
            progress.advance(task_id)
            table.add_row(
                artist,
                title,
                current,
                "[green]replacement found[/green]",
                suggested,
                _candidate_id(better),
            )

    if replacements:
        _store_candidates(replacements)
    summary = (
        f"Scanned {scanned} track(s); {len(flagged)} below {min_bitrate}kbps; "
        f"{found} replacement(s) found."
    )
    if skipped:
        summary += (
            f" {skipped} more below-threshold track(s) not searched "
            "(raise --limit or narrow --query)."
        )
    original_width = console.width
    console.width = max(original_width, 140)
    try:
        console.print(table)
        console.print(summary)
    finally:
        console.width = original_width

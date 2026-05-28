"""muzik soulseek — search and download via slskd."""

from __future__ import annotations

from pathlib import Path

import typer
from rich.table import Table

from muzik.config import DEFAULT_SOULSEEK_DIR
from muzik.commands.organize import organize_cmd
import muzik.core.cache as cache_mod
from muzik.core.sources.base import Candidate, DownloadRequest
from muzik.core.sources.soulseek import SoulseekError, SoulseekSource
from muzik.ui.console import console, err


app = typer.Typer(no_args_is_help=True, help="Search and download from Soulseek.")


def _source() -> SoulseekSource:
    return SoulseekSource()


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
    """Verify slskd connectivity and auth."""
    try:
        info = _source().check()
    except Exception as exc:
        err(f"[red]Soulseek/slskd check failed:[/red] {exc}")
        raise typer.Exit(1) from exc

    console.print("[green]slskd reachable[/green]")
    console.print(f"  URL: [dim]{info['url']}[/dim]")
    console.print(f"  Download dir: [dim]{info['download_dir']}[/dim]")
    console.print(f"  Auth valid: [dim]{info['auth_valid']}[/dim]")
    console.print(f"  Soulseek state: [dim]{info['server_state']}[/dim]")
    console.print(f"  Soulseek connected: [dim]{info['server_connected']}[/dim]")
    console.print(f"  Soulseek logged in: [dim]{info['server_logged_in']}[/dim]")
    if not info["auth_valid"]:
        raise typer.Exit(1)
    if not info["server_connected"] or not info["server_logged_in"]:
        err(
            "[red]slskd is not logged in to Soulseek.[/red] "
            "Set soulseek.username and soulseek.password in slskd.yml, then restart slskd."
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
        help="Local slskd download directory mapping.",
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
        choice = 1
        if not no_interactive:
            raw = typer.prompt("Candidate number", default="1")
            try:
                choice = int(raw)
            except ValueError as exc:
                err("[red]Invalid candidate number.[/red]")
                raise typer.Exit(1) from exc
        if choice < 1 or choice > min(limit, len(candidates)):
            err("[red]Candidate number out of range.[/red]")
            raise typer.Exit(1)

        candidate = candidates[choice - 1]
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

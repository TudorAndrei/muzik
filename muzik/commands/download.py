"""music download <url> — yt-dlp wrapper with post-download scenario report."""

from pathlib import Path
from typing import Optional

import typer
from rich.table import Table

from muzik.config import DEFAULT_DOWNLOAD_DIR
from muzik.core.chapters import find_chapters
from muzik.core.runner import run_streaming
from muzik.core.workflow.cancellation import CancellationToken
from muzik.core.sources.youtube import (
    build_download_command,
    find_audio_by_id,
    new_audio_files,
    youtube_id,
)
from muzik.ui.console import console, err


def _youtube_id(url: str) -> Optional[str]:
    return youtube_id(url)


def _scenario_label(chapters_count: int) -> str:
    if chapters_count:
        return f"[cyan]album[/cyan] [dim]({chapters_count} chapters)[/dim]"
    return "[yellow]single track[/yellow]"


def _download_audio(
    url: str,
    output: Path = DEFAULT_DOWNLOAD_DIR,
    format: str = "bestaudio",  # noqa: A002
    quality: str = "0",
    no_chapters: bool = False,
    archive_file: Optional[Path] = None,
    force: bool = False,
    cancellation: CancellationToken | None = None,
) -> None:
    """Download audio from YouTube, saving [ID] in the filename for cache compatibility.

    After downloading, reports whether each file is a single track or an album
    with chapter markers, so you know what to run next. When *force* is set, an
    already-downloaded file is fetched again instead of skipped.
    """
    output.mkdir(parents=True, exist_ok=True)

    cmd = build_download_command(
        url,
        format=format,
        quality=quality,
        no_chapters=no_chapters,
        archive_file=archive_file,
        force=force,
    )

    console.print(f"[bold cyan]Downloading:[/bold cyan] {url}")
    console.print(f"[dim]Output: {output.resolve()}[/dim]")

    # Snapshot existing files so we only report newly downloaded ones
    before = set(output.glob("*")) if output.exists() else set()

    if cancellation is None:
        rc = run_streaming(cmd, cwd=output, label="yt-dlp")
    else:
        rc = run_streaming(
            cmd,
            cwd=output,
            label="yt-dlp",
            cancellation=cancellation,
        )

    if rc != 0:
        err(f"[red]yt-dlp exited with code {rc}[/red]")
        raise typer.Exit(rc)

    console.print(f"[green]Download complete → {output.resolve()}[/green]")

    # ── Post-download scenario report ─────────────────────────────────────
    after = set(output.glob("*")) if output.exists() else set()
    new_audio = new_audio_files(before, after)

    # yt-dlp skips re-downloading existing files — fall back to ID-based lookup
    if not new_audio and output.exists():
        yt_id = _youtube_id(url)
        if yt_id:
            new_audio = find_audio_by_id(output, yt_id)

    if not new_audio:
        return

    table = Table(
        title="Downloaded files",
        show_header=True,
        header_style="bold cyan",
        border_style="dim",
    )
    table.add_column("File", overflow="fold")
    table.add_column("Scenario", width=28)
    table.add_column("Next step", overflow="fold")

    for af in new_audio:
        chapters = find_chapters(af)
        n = len(chapters)
        scenario = _scenario_label(n)
        if n:
            next_step = f"music split {af.name!r} [--review]"
        else:
            next_step = f"music organize {output}"
        table.add_row(af.name, scenario, f"[dim]{next_step}[/dim]")

    console.print(table)


def download_cmd(
    url: str = typer.Argument(..., help="YouTube URL (video, playlist, or album)."),
    output: Path = typer.Option(
        DEFAULT_DOWNLOAD_DIR,
        "--output",
        "-o",
        help="Directory to save downloaded files.",
    ),
    format: str = typer.Option(  # noqa: A002
        "bestaudio",
        "--format",
        "-f",
        help="yt-dlp format selector.",
    ),
    quality: str = typer.Option(
        "0",
        "--quality",
        "-q",
        help="Audio quality passed to yt-dlp (0 = best).",
    ),
    no_chapters: bool = typer.Option(
        False,
        "--no-chapters",
        help="Skip writing chapter info (no .info.json).",
    ),
    archive_file: Optional[Path] = typer.Option(
        None,
        "--archive-file",
        hidden=True,
        help="yt-dlp download archive for deduplication.",
    ),
) -> None:
    """Download audio from YouTube and report its next processing step."""
    _download_audio(url, output, format, quality, no_chapters, archive_file)

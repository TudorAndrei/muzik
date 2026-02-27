"""music download <url> — yt-dlp wrapper with post-download scenario report."""

import re
from pathlib import Path
from typing import Optional

import typer
from rich.table import Table

from muzik.config import AUDIO_EXTENSIONS, YTDLP_OUTPUT_TEMPLATE
from muzik.core.chapters import find_chapters
from muzik.core.runner import run_streaming
from muzik.ui.console import console, err


def _youtube_id(url: str) -> Optional[str]:
    m = re.search(r"(?:v=|youtu\.be/|/v/|/embed/)([A-Za-z0-9_-]{11})", url)
    return m.group(1) if m else None


def _scenario_label(chapters_count: int) -> str:
    if chapters_count:
        return f"[cyan]album[/cyan] [dim]({chapters_count} chapters)[/dim]"
    return "[yellow]single track[/yellow]"


def download_cmd(
    url: str = typer.Argument(..., help="YouTube URL (video, playlist, or album)."),
    output: Path = typer.Option(
        Path("./downloads"),
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
    """Download audio from YouTube, saving [ID] in the filename for cache compatibility.

    After downloading, reports whether each file is a single track or an album
    with chapter markers, so you know what to run next.
    """
    output.mkdir(parents=True, exist_ok=True)

    cmd = [
        "yt-dlp",
        "--format",
        format,
        "--extract-audio",
        "--audio-format",
        "flac",
        "--audio-quality",
        quality,
        "--embed-metadata",
        "--add-metadata",
        "--output",
        YTDLP_OUTPUT_TEMPLATE,
    ]
    if not no_chapters:
        cmd += ["--write-info-json", "--embed-chapters"]
    if archive_file:
        cmd += ["--download-archive", str(archive_file)]
    cmd.append(url)

    console.print(f"[bold cyan]Downloading:[/bold cyan] {url}")
    console.print(f"[dim]Output: {output.resolve()}[/dim]")

    # Snapshot existing files so we only report newly downloaded ones
    before = set(output.glob("*")) if output.exists() else set()

    rc = run_streaming(cmd, cwd=output, label="yt-dlp")

    if rc != 0:
        err(f"[red]yt-dlp exited with code {rc}[/red]")
        raise typer.Exit(rc)

    console.print(f"[green]Download complete → {output.resolve()}[/green]")

    # ── Post-download scenario report ─────────────────────────────────────
    after = set(output.glob("*")) if output.exists() else set()
    new_audio = sorted(
        f
        for f in (after - before)
        if f.is_file() and f.suffix.lower() in AUDIO_EXTENSIONS
    )

    # yt-dlp skips re-downloading existing files — fall back to ID-based lookup
    if not new_audio and output.exists():
        yt_id = _youtube_id(url)
        if yt_id:
            new_audio = sorted(
                f
                for f in output.iterdir()
                if f.is_file()
                and f.suffix.lower() in AUDIO_EXTENSIONS
                and f"[{yt_id}]" in f.name
            )

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

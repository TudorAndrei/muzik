"""music split <path> — ffmpeg chapter splitter with optional interactive review."""

from pathlib import Path
from typing import Optional

import typer
from rich.progress import (
    BarColumn,
    Progress,
    SpinnerColumn,
    TextColumn,
    TimeElapsedColumn,
)

from muzik.core import cache as cache_mod
from muzik.core.audio import extract_metadata
from muzik.core.chapters import find_chapters, safe_filename
from muzik.core.splitter import SplitError, split_audio
from muzik.core.workflow.cancellation import CancellationToken
from muzik.ui.chapter_editor import display_chapter_table, edit_chapters
from muzik.ui.console import console, err


# ---------------------------------------------------------------------------
# Command
# ---------------------------------------------------------------------------


def _split_audio(
    path: Path,
    review: bool = False,
    jobs: int = 0,
    output: Optional[Path] = None,
    keep_source: bool = False,
    force: bool = False,
    cancellation: CancellationToken | None = None,
) -> None:
    """Split an audio file into tracks by chapter, with CLI review and progress.

    The actual splitting (ffmpeg, cache, cover art, cleanup) is done by the one
    engine in ``muzik.core.splitter``; this wrapper only adds the interactive
    review, the header, and the progress bar.
    """
    cancellation = cancellation or CancellationToken()
    cancellation.raise_if_cancelled()
    if not path.exists():
        err(f"[red]File not found: {path}[/red]")
        raise typer.Exit(1)

    chapters = find_chapters(path)
    if not chapters:
        err(
            "[red]No chapters found.[/red] "
            "Expected a [dim].chapters.txt[/dim] sidecar "
            "or [dim].info.json[/dim] with a chapters array."
        )
        raise typer.Exit(1)

    # Optional review / edit loop
    if review:
        chapters = edit_chapters(chapters)
        if chapters is None:
            raise typer.Exit(0)
    else:
        display_chapter_table(chapters, title=f"Chapters — {path.name}")

    metadata = extract_metadata(path)

    # Report a cache hit nicely before delegating; the engine also honours it.
    base = path.with_suffix("")
    txt_path = base.with_suffix(".chapters.txt")
    if not force and txt_path.exists():
        cache_key = cache_mod.split_cache_key(path, txt_path)
        cached = cache_mod.get(cache_key)
        if cached and Path(cached.strip()).exists():
            console.print(
                f"[green]Already split (cached).[/green] Output: {cached.strip()}"
            )
            raise typer.Exit(0)

    if output is None:
        out_dir = path.parent.parent / "splits" / safe_filename(metadata["album"])
    else:
        out_dir = output

    console.print(f"[bold]Splitting[/bold] {len(chapters)} tracks")
    console.print(
        f"[dim]  Artist: {metadata['artist']} | "
        f"Album: {metadata['album']} | "
        f"Year: {metadata['year']}[/dim]"
    )
    console.print(f"[dim]  Output: {out_dir}[/dim]")

    with Progress(
        SpinnerColumn(),
        TextColumn("[progress.description]{task.description}"),
        BarColumn(),
        TextColumn("[progress.percentage]{task.percentage:>3.0f}%"),
        TimeElapsedColumn(),
        console=console,
    ) as progress:
        task_id = progress.add_task("Splitting…", total=len(chapters))
        try:
            split_audio(
                path,
                chapters,
                output=out_dir,
                jobs=jobs,
                keep_source=keep_source,
                force=force,
                cancellation=cancellation,
                on_progress=lambda _title, _ok: progress.advance(task_id),
            )
        except SplitError as exc:
            err(f"[red]{exc}[/red]")
            raise typer.Exit(1) from exc

    console.print(f"[green]✓ {len(chapters)} tracks → {out_dir}[/green]")
    if not keep_source:
        console.print("[dim]Source files removed.[/dim]")


def split_cmd(
    path: Path = typer.Argument(..., help="Audio file to split."),
    review: bool = typer.Option(
        False,
        "--review",
        "-r",
        help="Show chapter table and open $EDITOR before splitting.",
    ),
    jobs: int = typer.Option(
        0,
        "--jobs",
        "-j",
        help="Parallel ffmpeg jobs (0 = auto-detect from CPU count).",
    ),
    output: Optional[Path] = typer.Option(
        None,
        "--output",
        "-o",
        help="Output directory (default: <audio_parent>/../splits/<album>).",
    ),
    keep_source: bool = typer.Option(
        False,
        "--keep-source",
        help="Keep original audio and sidecar files after splitting.",
    ),
    force: bool = typer.Option(
        False,
        "--force",
        "-f",
        help="Ignore split cache and re-split even if already done.",
    ),
) -> None:
    """Split an audio file into individual tracks using chapter markers."""
    _split_audio(path, review, jobs, output, keep_source, force)

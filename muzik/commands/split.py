"""music split <path> — ffmpeg chapter splitter with optional interactive review."""

import os
import shutil
import subprocess
from concurrent.futures import ThreadPoolExecutor, as_completed
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
from muzik.core.chapters import Chapter, find_chapters, safe_filename
from muzik.core.workflow.cancellation import CancellationToken
from muzik.ui.chapter_editor import display_chapter_table, edit_chapters
from muzik.ui.console import console, err


# ---------------------------------------------------------------------------
# Internal helpers
# ---------------------------------------------------------------------------


def _split_track(
    audio_path: Path,
    output_dir: Path,
    chapter: Chapter,
    metadata: dict,
    track_count: int,
) -> tuple[bool, str]:
    """Split one chapter from *audio_path* using ffmpeg.

    Returns (success, chapter_title).
    """
    safe_title = safe_filename(chapter.title)
    out_path = output_dir / f"{chapter.index:02d}-{safe_title}{audio_path.suffix}"

    cmd = [
        "ffmpeg",
        "-i",
        str(audio_path),
        "-nostdin",
        "-y",
        "-ss",
        chapter.start_ts,
    ]
    if chapter.end is not None:
        end_ts = chapter.end_ts
        if end_ts is not None:
            cmd.extend(["-to", end_ts])

    cmd += [
        "-vn",
        "-c:a",
        "copy",
        "-metadata",
        f"title={chapter.title}",
        "-metadata",
        f"artist={metadata['artist']}",
        "-metadata",
        f"album={metadata['album']}",
        "-metadata",
        f"date={metadata['year']}",
        "-metadata",
        f"track={chapter.index}/{track_count}",
        str(out_path),
    ]

    result = subprocess.run(cmd, capture_output=True)
    return result.returncode == 0, chapter.title


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
    """Split an audio file into individual tracks using chapter markers."""
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

    # Check split cache before touching any output directory. A valid cache hit
    # must preserve the prior split output and its source audio.
    base = path.with_suffix("")
    txt_path = base.with_suffix(".chapters.txt")
    cache_key: Optional[str] = None
    if txt_path.exists():
        cache_key = cache_mod.split_cache_key(path, txt_path)
        cached = cache_mod.get(cache_key)
        if not force and cached and Path(cached.strip()).exists():
            console.print(
                f"[green]Already split (cached).[/green] Output: {cached.strip()}"
            )
            raise typer.Exit(0)

    # Determine output directory only after cache hits have returned. Existing
    # populated output is never replaced without an explicit --force.
    if output is None:
        album_slug = safe_filename(metadata["album"])
        out_dir = path.parent.parent / "splits" / album_slug
    else:
        out_dir = output

    if out_dir.exists():
        if not out_dir.is_dir():
            err(f"[red]Output path is not a directory:[/red] {out_dir}")
            raise typer.Exit(1)
        if any(out_dir.iterdir()):
            if not force:
                err(
                    "[red]Output directory is not empty.[/red] "
                    "Use [bold]--force[/bold] to replace it."
                )
                raise typer.Exit(1)
            cancellation.raise_if_cancelled()
            shutil.rmtree(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    # Determine parallelism
    if jobs <= 0:
        cpu = os.cpu_count() or 4
        jobs = max(2, min(8, cpu // 2))

    console.print(
        f"[bold]Splitting[/bold] {len(chapters)} tracks "
        f"with {jobs} parallel job{'s' if jobs != 1 else ''}"
    )
    console.print(
        f"[dim]  Artist: {metadata['artist']} | "
        f"Album: {metadata['album']} | "
        f"Year: {metadata['year']}[/dim]"
    )
    console.print(f"[dim]  Output: {out_dir}[/dim]")

    failed: list[str] = []

    with Progress(
        SpinnerColumn(),
        TextColumn("[progress.description]{task.description}"),
        BarColumn(),
        TextColumn("[progress.percentage]{task.percentage:>3.0f}%"),
        TimeElapsedColumn(),
        console=console,
    ) as progress:
        task_id = progress.add_task("Splitting…", total=len(chapters))

        with ThreadPoolExecutor(max_workers=jobs) as pool:
            futures = {
                pool.submit(
                    _split_track, path, out_dir, ch, metadata, len(chapters)
                ): ch
                for ch in chapters
            }
            for future in as_completed(futures):
                ok, title = future.result()
                if not ok:
                    failed.append(title)
                progress.advance(task_id)
                cancellation.raise_if_cancelled()

    if failed:
        err(f"[red]Failed to split {len(failed)} track(s):[/red]")
        for title in failed:
            err(f"  [red]• {title}[/red]")
        raise typer.Exit(1)

    console.print(f"[green]✓ {len(chapters)} tracks → {out_dir}[/green]")

    # Update split cache
    cancellation.raise_if_cancelled()
    if cache_key:
        cache_mod.set(cache_key, str(out_dir))

    # Clean up source files unless --keep-source
    if not keep_source:
        cancellation.raise_if_cancelled()
        path.unlink(missing_ok=True)
        for ext in (".chapters.txt", ".info.json", ".metadata.txt"):
            sidecar = base.with_suffix(ext)
            sidecar.unlink(missing_ok=True)
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

"""Presentation-free ffmpeg chapter splitting."""

from __future__ import annotations

import os
import shutil
import subprocess
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

from muzik.core import cache as cache_mod
from muzik.core.audio import extract_metadata
from muzik.core.chapters import Chapter, safe_filename
from muzik.core.workflow.cancellation import CancellationToken


class SplitError(RuntimeError):
    """Raised when a split request cannot complete safely."""


def split_audio(
    path: Path,
    chapters: list[Chapter],
    *,
    output: Path,
    jobs: int = 0,
    keep_source: bool = False,
    force: bool = False,
    cancellation: CancellationToken | None = None,
) -> Path:
    """Split *path* by supplied chapters and return its output directory."""
    cancellation = cancellation or CancellationToken()
    cancellation.raise_if_cancelled()
    if not path.exists():
        raise SplitError(f"File not found: {path}")
    if not chapters:
        raise SplitError("No chapters found.")

    metadata = extract_metadata(path)
    base = path.with_suffix("")
    chapter_path = base.with_suffix(".chapters.txt")
    cache_key: str | None = None
    if chapter_path.exists():
        cache_key = cache_mod.split_cache_key(path, chapter_path)
        cached = cache_mod.get(cache_key)
        if not force and cached and Path(cached.strip()).exists():
            return Path(cached.strip())

    if output.exists():
        if not output.is_dir():
            raise SplitError(f"Output path is not a directory: {output}")
        if any(output.iterdir()):
            if not force:
                raise SplitError("Output directory is not empty; use --force.")
            cancellation.raise_if_cancelled()
            shutil.rmtree(output)
    output.mkdir(parents=True, exist_ok=True)

    workers = jobs
    if workers <= 0:
        workers = max(2, min(8, (os.cpu_count() or 4) // 2))

    failures: list[str] = []
    with ThreadPoolExecutor(max_workers=workers) as pool:
        futures = {
            pool.submit(
                _split_track, path, output, chapter, metadata, len(chapters)
            ): chapter
            for chapter in chapters
        }
        for future in as_completed(futures):
            ok, title = future.result()
            if not ok:
                failures.append(title)
            cancellation.raise_if_cancelled()
    if failures:
        raise SplitError(
            f"Failed to split {len(failures)} track(s): {', '.join(failures)}"
        )

    cancellation.raise_if_cancelled()
    _place_cover(base, output)
    if cache_key:
        cache_mod.set(cache_key, str(output))
    if not keep_source:
        cancellation.raise_if_cancelled()
        path.unlink(missing_ok=True)
        for extension in (
            ".chapters.txt",
            ".info.json",
            ".metadata.txt",
            *_THUMB_EXTS,
        ):
            base.with_suffix(extension).unlink(missing_ok=True)
    return output


# Thumbnail extensions yt-dlp may leave beside a download, best first.
_THUMB_EXTS = (".jpg", ".jpeg", ".png", ".webp")


def _place_cover(base: Path, output: Path) -> None:
    """Copy a downloaded thumbnail into the album folder as cover art.

    Beets' fetchart picks up a ``cover.*`` image on import, so the album gets a
    cover even when MusicBrainz has none.
    """
    for extension in _THUMB_EXTS:
        thumb = base.with_suffix(extension)
        if thumb.exists():
            try:
                shutil.copyfile(thumb, output / f"cover{extension}")
            except OSError:
                pass
            return


def _split_track(
    audio_path: Path,
    output_dir: Path,
    chapter: Chapter,
    metadata: dict,
    track_count: int,
) -> tuple[bool, str]:
    output_path = output_dir / (
        f"{chapter.index:02d}-{safe_filename(chapter.title)}{audio_path.suffix}"
    )
    command = [
        "ffmpeg",
        "-i",
        str(audio_path),
        "-nostdin",
        "-y",
        "-ss",
        chapter.start_ts,
    ]
    if chapter.end is not None and chapter.end_ts is not None:
        command.extend(["-to", chapter.end_ts])
    command.extend(
        [
            "-vn",
            "-c:a",
            "copy",
            # Drop the source's embedded tags first; for Opus/Vorbis a bare
            # -metadata does not override them, so the track would keep the
            # whole-video title and uploader.
            "-map_metadata",
            "-1",
            "-metadata",
            f"title={chapter.title}",
            "-metadata",
            f"artist={metadata['artist']}",
            "-metadata",
            f"albumartist={metadata['artist']}",
            "-metadata",
            f"album={metadata['album']}",
            "-metadata",
            f"date={metadata['year']}",
            "-metadata",
            f"track={chapter.index}/{track_count}",
            str(output_path),
        ]
    )
    result = subprocess.run(command, capture_output=True)
    return result.returncode == 0, chapter.title

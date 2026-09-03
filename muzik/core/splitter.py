"""Presentation-free ffmpeg chapter splitting."""

from __future__ import annotations

from collections.abc import Callable
import os
import shutil
import subprocess
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

from muzik.core import cache as cache_mod
from muzik.core.audio import extract_metadata
from muzik.core.chapters import (
    Chapter,
    parse_artist_title,
    safe_filename,
    sidecar_path,
    strip_featured,
)
from muzik.core.metadata import find_muzik_metadata, write_muzik_metadata
from muzik.core.workflow.cancellation import CancellationToken


# beets files an album under "Various Artists" and sets its comp flag when the
# album artist is this name; used for compilations of per-track artists.
VARIOUS_ARTISTS = "Various Artists"


# Called once per finished track with (title, ok) so a UI can show progress.
ProgressCallback = Callable[[str, bool], None]


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
    compilation: bool = False,
    cancellation: CancellationToken | None = None,
    on_progress: ProgressCallback | None = None,
) -> Path:
    """Split *path* by supplied chapters and return its output directory.

    When *compilation* is set, each track title is parsed as "Artist - Song";
    the per-track artist is kept, the album artist becomes "Various Artists",
    and the track is marked a compilation so beets files it correctly.
    """
    cancellation = cancellation or CancellationToken()
    cancellation.raise_if_cancelled()
    if not path.exists():
        raise SplitError(f"File not found: {path}")
    if not chapters:
        raise SplitError("No chapters found.")

    metadata = extract_metadata(path)
    source_meta = find_muzik_metadata(path)
    source_id = source_meta.get("source_id") if source_meta else None
    chapter_path = sidecar_path(path, ".chapters.txt")
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
                _split_track,
                path,
                output,
                chapter,
                metadata,
                len(chapters),
                compilation,
                source_id,
            ): chapter
            for chapter in chapters
        }
        for future in as_completed(futures):
            ok, title = future.result()
            if not ok:
                failures.append(title)
            if on_progress is not None:
                on_progress(title, ok)
            cancellation.raise_if_cancelled()
    if failures:
        raise SplitError(
            f"Failed to split {len(failures)} track(s): {', '.join(failures)}"
        )

    cancellation.raise_if_cancelled()
    _place_cover(path, output)
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
            sidecar_path(path, extension).unlink(missing_ok=True)
    return output


# Thumbnail extensions yt-dlp may leave beside a download, best first.
_THUMB_EXTS = (".jpg", ".jpeg", ".png", ".webp")


def _place_cover(audio_path: Path, output: Path) -> None:
    """Copy a downloaded thumbnail into the album folder as cover art.

    Beets' fetchart picks up a ``cover.*`` image on import, so the album gets a
    cover even when MusicBrainz has none.
    """
    for extension in _THUMB_EXTS:
        thumb = sidecar_path(audio_path, extension)
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
    compilation: bool = False,
    source_id: str | None = None,
) -> tuple[bool, str]:
    # For a compilation, identify each song's own artist from its
    # "Artist - Song" title; the album artist becomes "Various Artists".
    if compilation:
        parsed_artist, title = parse_artist_title(chapter.title)
        artist = parsed_artist or metadata["artist"]
        albumartist = VARIOUS_ARTISTS
    else:
        title = chapter.title
        artist = metadata["artist"]
        albumartist = metadata["artist"]

    # Keep only the song name in the title; move a "feat." credit into the
    # artist field as "Main feat. X", the form beets and MusicBrainz use.
    title, featured = strip_featured(title)
    if featured:
        artist = f"{artist} feat. {', '.join(featured)}"

    output_path = output_dir / (
        f"{chapter.index:02d}-{safe_filename(title)}{audio_path.suffix}"
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
            f"title={title}",
            "-metadata",
            f"artist={artist}",
            "-metadata",
            f"albumartist={albumartist}",
            "-metadata",
            f"album={metadata['album']}",
            "-metadata",
            f"date={metadata['year']}",
            "-metadata",
            f"track={chapter.index}/{track_count}",
            # Mark a compilation so beets sets its comp flag and groups the
            # album under Various Artists despite the differing track artists.
            "-metadata",
            f"compilation={1 if compilation else 0}",
            str(output_path),
        ]
    )
    result = subprocess.run(command, capture_output=True)
    ok = result.returncode == 0
    if ok and source_id:
        # The muzik_source beets plugin reads this at import time, before
        # the file is moved into the library, to record an exact video-id
        # match for the Watchlist page instead of guessing from the title.
        write_muzik_metadata(output_path, {"source_id": source_id})
    return ok, chapter.title

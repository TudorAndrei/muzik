"""Downloaded-audio inventory for the output folder.

Downloads keep the YouTube id in the filename (``Title [dQw4w9WgXcQ].m4a``).
This module reads that folder back into a list of items and seeds a yt-dlp
download archive from it, so an id already on disk is never fetched again — even
when it later appears inside a playlist.
"""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
import re

from muzik.config import AUDIO_EXTENSIONS


# YouTube ids are 11 characters of the URL-safe alphabet, wrapped in brackets.
_ID_IN_NAME = re.compile(r"\[([A-Za-z0-9_-]{11})\]")


@dataclass(frozen=True)
class DownloadedItem:
    """One audio file already present in the output folder."""

    path: Path
    title: str
    youtube_id: str | None
    ext: str
    size: int
    mtime: float


def youtube_id_from_name(name: str) -> str | None:
    """Return the bracketed YouTube id in a filename, if present."""
    match = _ID_IN_NAME.search(name)
    return match.group(1) if match else None


def _title_from_name(stem: str) -> str:
    """Strip a trailing ``[id]`` marker to get a readable title."""
    return _ID_IN_NAME.sub("", stem).strip().rstrip("-").strip() or stem


def scan_downloads(directory: Path) -> list[DownloadedItem]:
    """Return the audio inventory of *directory*, sorted by title.

    Only top-level audio files are listed; the folder is flat by design.
    """
    if not directory.exists():
        return []
    items: list[DownloadedItem] = []
    for file in directory.iterdir():
        if not file.is_file() or file.suffix.lower() not in AUDIO_EXTENSIONS:
            continue
        stat = file.stat()
        items.append(
            DownloadedItem(
                path=file,
                title=_title_from_name(file.stem),
                youtube_id=youtube_id_from_name(file.name),
                ext=file.suffix.lstrip(".").lower(),
                size=stat.st_size,
                mtime=stat.st_mtime,
            )
        )
    return sorted(items, key=lambda item: item.title.lower())


def downloaded_ids(directory: Path) -> set[str]:
    """Return every YouTube id found in *directory* filenames."""
    if not directory.exists():
        return set()
    ids: set[str] = set()
    for file in directory.iterdir():
        if not file.is_file() or file.suffix.lower() not in AUDIO_EXTENSIONS:
            continue
        found = youtube_id_from_name(file.name)
        if found:
            ids.add(found)
    return ids


def _archive_ids(archive_file: Path) -> set[str]:
    ids: set[str] = set()
    if archive_file.exists():
        for line in archive_file.read_text().splitlines():
            parts = line.strip().split()
            if len(parts) >= 2:
                ids.add(parts[1])
    return ids


def seed_archive_from_downloads(archive_file: Path, directory: Path) -> int:
    """Add ``youtube <id>`` lines for ids on disk but missing from the archive.

    yt-dlp skips any id already in its download archive, so seeding it from the
    output folder makes downloads idempotent across playlists. Returns the count
    of new lines written.
    """
    present = downloaded_ids(directory)
    if not present:
        return 0
    known = _archive_ids(archive_file)
    new_ids = sorted(present - known)
    if not new_ids:
        return 0
    archive_file.parent.mkdir(parents=True, exist_ok=True)
    with archive_file.open("a") as fh:
        fh.writelines(f"youtube {video_id}\n" for video_id in new_ids)
    return len(new_ids)


def human_size(num_bytes: int) -> str:
    """Format a byte count with a binary unit suffix."""
    value = float(num_bytes)
    for unit in ("B", "KB", "MB", "GB"):
        if value < 1024:
            return f"{value:.1f} {unit}"
        value /= 1024
    return f"{value:.1f} TB"

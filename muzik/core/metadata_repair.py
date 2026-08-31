"""Repair placeholder tags in split albums created by older muzik versions."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
import re

from mediafile import MediaFile, UnreadableFileError

from muzik.config import AUDIO_EXTENSIONS
from muzik.core.audio import _parse_title


_VIDEO_ID_SUFFIX = re.compile(r"\s*\[[A-Za-z0-9_-]{11}\]\s*$")
_FULL_ALBUM_SUFFIX = re.compile(
    r"\s*[\[(]\s*full\s+album\s*[\])]\s*$",
    re.IGNORECASE,
)
_PLACEHOLDERS = {
    "",
    "[unknown]",
    "artist unknown",
    "unknown",
    "unknown album",
    "unknown artist",
    "unknown title",
}


@dataclass(frozen=True, slots=True)
class MetadataRepairResult:
    """Summary of one placeholder-tag repair."""

    updated_files: int
    artist: str = ""
    album: str = ""
    year: str = ""


def repair_placeholder_album_tags(directory: Path) -> MetadataRepairResult:
    """Fill placeholder album tags from a YouTube split-folder name.

    Existing real tags stay unchanged. This repairs split folders that an old
    muzik version created before it kept the YouTube title metadata.
    """
    if not directory.is_dir():
        return MetadataRepairResult(0)
    artist, album, year = _metadata_from_directory_name(directory.name)
    if not artist or not album:
        return MetadataRepairResult(0)

    updated = 0
    for path in sorted(directory.iterdir()):
        if not path.is_file() or path.suffix.lower() not in AUDIO_EXTENSIONS:
            continue
        try:
            media = MediaFile(path)
        except OSError, UnreadableFileError:
            continue

        changed = False
        if _is_placeholder(media.artist):
            media.artist = artist
            changed = True
        if _is_placeholder(media.albumartist):
            media.albumartist = artist
            changed = True
        if _is_placeholder(media.album):
            media.album = album
            changed = True
        if year and _is_placeholder(media.year):
            media.year = int(year)
            changed = True
        if changed:
            media.save()
            updated += 1

    return MetadataRepairResult(updated, artist, album, year)


def _metadata_from_directory_name(name: str) -> tuple[str, str, str]:
    title = _VIDEO_ID_SUFFIX.sub("", name)
    title = _FULL_ALBUM_SUFFIX.sub("", title)
    return _parse_title(title)


def _is_placeholder(value: object) -> bool:
    if value is None:
        return True
    return str(value).strip().casefold() in _PLACEHOLDERS

"""Find imported music in the native library."""

from __future__ import annotations

import os
from pathlib import Path

from muzik.core.audio import _parse_title
from muzik.core.native_library import NativeLibrary

SOURCE_ID_FIELD = "muzik_source_id"


def resolve_item_path(directory: str, raw_path: bytes) -> Path:
    """Resolve a stored item path against the library directory."""
    decoded = os.fsdecode(raw_path)
    if not os.path.isabs(decoded):
        decoded = os.path.join(directory, decoded)
    return Path(decoded)


def find_path_by_source_id(video_id: str, library: NativeLibrary) -> Path | None:
    """Find the item with an exact source ID."""
    directory = os.fsdecode(library.directory)
    for item in library.items(f"{SOURCE_ID_FIELD}:{video_id}"):
        if str(item.get(SOURCE_ID_FIELD) or "") == video_id:
            return resolve_item_path(directory, item.path)
    return None


def find_organized_path(title: str, library: NativeLibrary) -> Path | None:
    """Find an album with the artist and album parsed from a video title."""
    artist, album, _year = _parse_title(title)
    artist = artist.strip().lower()
    album = album.strip().lower()
    if not artist or not album:
        return None

    directory = os.fsdecode(library.directory)
    for candidate in library.albums():
        if str(candidate.albumartist or "").strip().lower() != artist:
            continue
        if str(candidate.album or "").strip().lower() != album:
            continue
        items = candidate.items()
        if items:
            return resolve_item_path(directory, items[0].path)
    return None

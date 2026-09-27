"""Best-effort matching between a video and an existing Beets album."""

from __future__ import annotations

import os
from pathlib import Path

from beets.library import Library

from muzik.beets_plugins.muzik_source import FIELD_NAME
from muzik.core.audio import _parse_title
from muzik.core.native_library import NativeLibrary, ShadowLibrary


def resolve_item_path(directory: str, raw_path: bytes) -> Path:
    """Turn a Beets item's stored ``path`` into a real absolute path.

    A move-mode import stores the path relative to the library directory;
    resolve it the same way ``_item_fullpath`` does in
    ``muzik/core/beets/importer.py``.
    """
    decoded = os.fsdecode(raw_path)
    if not os.path.isabs(decoded):
        decoded = os.path.join(directory, decoded)
    return Path(decoded)


def find_path_by_source_id(
    video_id: str, library: Library | NativeLibrary | ShadowLibrary
) -> Path | None:
    """Return the path of a Beets item tagged with this exact video id.

    The `muzik_source` beets plugin records the id at import time (see
    ``muzik/beets_plugins/muzik_source.py``); this only finds items
    imported since that plugin existed. Beets' query syntax matches a
    flexible field by substring, so results are re-checked for an exact
    value before accepting one, rather than trusting the query alone.
    """
    directory = os.fsdecode(library.directory)
    for item in library.items(f"{FIELD_NAME}:{video_id}"):
        if str(item.get(FIELD_NAME) or "") != video_id:
            continue
        return resolve_item_path(directory, item.path)
    return None


def find_organized_path(
    title: str, library: Library | NativeLibrary | ShadowLibrary
) -> Path | None:
    """Return the path of an already-imported Beets album matching *title*.

    Best-effort only: parses ``title`` the same way muzik would parse a
    YouTube video title it was about to organize itself
    (``"Artist - Album (Year)"``), then looks for an exact, normalized
    match against ``Album.albumartist``/``Album.album``. A title with no
    identifiable artist/album half is not looked up at all — matching on
    the album name alone risks a wrong match. Returns ``None`` on no match.
    """
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
        items = list(candidate.items())
        if not items:
            continue
        return resolve_item_path(directory, items[0].path)
    return None

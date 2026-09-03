"""Best-effort matching between a video title and an existing Beets album."""

from __future__ import annotations

import os
from pathlib import Path

from beets.library import Library

from muzik.core.audio import _parse_title


def find_organized_path(title: str, library: Library) -> Path | None:
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
        raw_path = os.fsdecode(items[0].path)
        if not os.path.isabs(raw_path):
            raw_path = os.path.join(directory, raw_path)
        return Path(raw_path)
    return None

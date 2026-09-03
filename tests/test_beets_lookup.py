"""Tests for matching a video title against an existing Beets album."""

import os
from pathlib import Path

from beets.library import Item, Library

from muzik.core.beets.lookup import find_organized_path, find_path_by_source_id


def _library(tmp_path: Path) -> tuple[Library, Path]:
    music = tmp_path / "music"
    music.mkdir()
    lib = Library(str(tmp_path / "lib.db"), str(music))
    return lib, music


def _add_album(
    lib: Library,
    path: Path,
    *,
    artist: str,
    album: str,
) -> None:
    item = Item(
        path=os.fsencode(str(path)),
        title="Track 1",
        artist=artist,
        album=album,
        albumartist=artist,
    )
    lib.add(item)
    lib.add_album([item])


def test_matches_a_full_album_title_case_and_noise_insensitively(
    tmp_path: Path,
) -> None:
    lib, music = _library(tmp_path)
    track = music / "Artist" / "01 Track 1.mp3"
    track.parent.mkdir()
    track.write_bytes(b"x")
    _add_album(lib, track, artist="ETNOBOTANIKA", album="KOSMOBOTANIKA")

    found = find_organized_path("etnobotanika - KOSMOBOTANIKA (Full Album)", lib)

    assert found == track


def test_no_match_returns_none(tmp_path: Path) -> None:
    lib, _music = _library(tmp_path)

    assert find_organized_path("Some Artist - Some Album", lib) is None


def test_unparsable_title_returns_none_without_a_library_scan(
    tmp_path: Path, monkeypatch
) -> None:
    lib, _music = _library(tmp_path)

    def fail_if_called():
        raise AssertionError("albums() should not be called for an unparsable title")

    monkeypatch.setattr(lib, "albums", fail_if_called)

    assert find_organized_path("Garbled Title With No Separator", lib) is None


def _add_item_with_source_id(lib: Library, path: Path, source_id: str) -> None:
    item = Item(path=os.fsencode(str(path)), title="Track 1")
    item.muzik_source_id = source_id
    lib.add(item)
    item.store()


def test_find_path_by_source_id_matches_exactly(tmp_path: Path) -> None:
    lib, music = _library(tmp_path)
    track = music / "Track 1.mp3"
    track.write_bytes(b"x")
    _add_item_with_source_id(lib, track, "abc-def_123")

    assert find_path_by_source_id("abc-def_123", lib) == track


def test_find_path_by_source_id_does_not_match_a_substring(tmp_path: Path) -> None:
    lib, music = _library(tmp_path)
    track = music / "Track 1.mp3"
    track.write_bytes(b"x")
    _add_item_with_source_id(lib, track, "abc-def_123")

    # Beets' query syntax matches a flex field by substring by default; a
    # shorter id that happens to be a substring must not count as a match.
    assert find_path_by_source_id("abc", lib) is None


def test_find_path_by_source_id_returns_none_without_a_match(tmp_path: Path) -> None:
    lib, _music = _library(tmp_path)

    assert find_path_by_source_id("nope", lib) is None

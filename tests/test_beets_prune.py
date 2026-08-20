"""Tests for pruning library items whose files have moved away."""

import os
from pathlib import Path

import pytest
from beets.library import Item, Library

from muzik.core.beets import importer


def _library(tmp_path: Path) -> tuple[Library, Path]:
    music = tmp_path / "music"
    music.mkdir()
    lib = Library(str(tmp_path / "lib.db"), str(music))
    return lib, music


def _add(lib: Library, path: Path, title: str) -> None:
    item = Item(path=os.fsencode(str(path)), title=title)
    lib.add(item)


def test_prune_removes_only_missing_items(tmp_path: Path, monkeypatch) -> None:
    lib, music = _library(tmp_path)
    present = music / "present.mp3"
    present.write_bytes(b"x")
    _add(lib, present, "present")
    _add(lib, music / "gone.mp3", "missing")

    monkeypatch.setattr(importer, "open_library", lambda config_path=None: lib)
    removed = importer.prune_missing_items()

    assert removed == 1
    assert {item.title for item in lib.items()} == {"present"}


def test_prune_resolves_relative_paths(tmp_path: Path, monkeypatch) -> None:
    # A move-tagged item is stored relative to the library directory.
    lib, music = _library(tmp_path)
    (music / "Artist").mkdir()
    real = music / "Artist" / "01 Song.mp3"
    real.write_bytes(b"x")
    _add(lib, Path("Artist/01 Song.mp3"), "relative-present")  # not absolute

    monkeypatch.setattr(importer, "open_library", lambda config_path=None: lib)
    removed = importer.prune_missing_items()

    assert removed == 0  # resolved against the library dir, the file exists
    assert {item.title for item in lib.items()} == {"relative-present"}


def test_prune_aborts_when_most_items_missing(tmp_path: Path, monkeypatch) -> None:
    lib, music = _library(tmp_path)
    ok = music / "ok.mp3"
    ok.write_bytes(b"x")
    _add(lib, ok, "ok")
    for name in ("g1", "g2", "g3"):
        _add(lib, music / f"{name}.mp3", name)  # 3 of 4 missing

    monkeypatch.setattr(importer, "open_library", lambda config_path=None: lib)
    with pytest.raises(importer.PruneAborted) as exc:
        importer.prune_missing_items()

    assert exc.value.missing == 3
    assert exc.value.total == 4
    # Nothing was removed because the safeguard tripped.
    assert len(list(lib.items())) == 4

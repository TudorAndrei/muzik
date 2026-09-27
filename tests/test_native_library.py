"""Read a beets fixture database through the native library switch."""

from pathlib import Path
from types import SimpleNamespace
from typing import Any, cast

from beets.library import Library

from muzik.commands import soulseek
from muzik.core.beets.lookup import find_organized_path, find_path_by_source_id
from muzik.core.native_library import (
    NativeLibrary,
    ShadowLibrary,
    open_library_for_reads,
)
from muzik.core import watchlist


FIXTURE_DB = (
    Path(__file__).resolve().parents[1]
    / "rust/crates/muzik-library/tests/fixtures/library.db"
)


def _config(tmp_path: Path) -> Path:
    config = tmp_path / "beets.yaml"
    config.write_text(
        f"library: {FIXTURE_DB}\ndirectory: /fixture/music\n", encoding="utf-8"
    )
    return config


def test_native_lookup_reads_beets_items_and_albums(
    tmp_path: Path, monkeypatch
) -> None:
    monkeypatch.setenv("MUZIK_NATIVE_LIBRARY", "native")
    library = open_library_for_reads(_config(tmp_path))
    assert isinstance(library, NativeLibrary)
    assert [item.id for item in library.items("artist:Artist")] == [1]
    path = Path("/fixture/music/Artist/Album/01 Track.mp3")
    assert find_path_by_source_id("video-123", library) == path
    assert find_organized_path("Artist - Album", library) == path


def test_shadow_read_uses_beets_result_after_native_error(caplog) -> None:
    expected = SimpleNamespace(id=1, path=b"Track.mp3", title="Track")
    beets = SimpleNamespace(directory="/fixture/music", items=lambda query: [expected])

    def fail(_query):
        raise RuntimeError("native read failed")

    native = SimpleNamespace(items=fail)
    library = ShadowLibrary(cast(Library, beets), cast(NativeLibrary, native))
    assert library.items("title:Track") == [expected]
    assert "Native library item read failed" in caplog.text


def test_watchlist_selects_native_library(monkeypatch, tmp_path: Path) -> None:
    expected = object()
    monkeypatch.setattr(watchlist, "get_native_settings", lambda: {"library": "native"})
    monkeypatch.setattr(watchlist, "open_library_for_reads", lambda config: expected)
    assert watchlist._open_beets_library(tmp_path / "beets.yaml") is expected


def test_soulseek_check_selects_native_library(monkeypatch, tmp_path: Path) -> None:
    native = SimpleNamespace(directory=str(tmp_path), items=lambda query: [])
    monkeypatch.setattr(soulseek, "get_native_settings", lambda: {"library": "native"})
    monkeypatch.setattr(soulseek, "open_library_for_reads", lambda config: native)

    def fail_beets(_config: Path | None) -> Any:
        raise AssertionError("beets reader was called")

    monkeypatch.setattr(soulseek, "open_library", fail_beets)
    soulseek.check_library_cmd(
        query=None,
        min_bitrate=256,
        prefer="lossless",
        limit=10,
        config=None,
    )

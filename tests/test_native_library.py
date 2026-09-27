"""Read a beets fixture database through the native library switch."""

from pathlib import Path
import shutil
from types import SimpleNamespace
from muzik.commands import soulseek
from muzik.core.library_lookup import find_organized_path, find_path_by_source_id
from muzik.core import library_prune as importer
from muzik.core.native_library import (
    NativeLibrary,
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


def test_native_lookup_reads_beets_items_and_albums(tmp_path: Path) -> None:
    library = open_library_for_reads(_config(tmp_path))
    assert isinstance(library, NativeLibrary)
    assert [item.id for item in library.items("artist:Artist")] == [1]
    path = Path("/fixture/music/Artist/Album/01 Track.mp3")
    assert find_path_by_source_id("video-123", library) == path
    assert find_organized_path("Artist - Album", library) == path


def test_watchlist_selects_native_library(monkeypatch, tmp_path: Path) -> None:
    expected = object()
    monkeypatch.setattr(watchlist, "open_library_for_reads", lambda config: expected)
    assert watchlist._open_music_library(tmp_path / "beets.yaml") is expected


def test_soulseek_check_selects_native_library(monkeypatch, tmp_path: Path) -> None:
    native = SimpleNamespace(directory=str(tmp_path), items=lambda query: [])
    monkeypatch.setattr(soulseek, "open_library_for_reads", lambda config: native)
    soulseek.check_library_cmd(
        query=None,
        min_bitrate=256,
        prefer="lossless",
        limit=10,
        config=None,
    )


def test_native_prune_checks_fraction_and_backs_up_database(
    tmp_path: Path,
) -> None:
    database = tmp_path / "library.db"
    shutil.copyfile(FIXTURE_DB, database)
    config = tmp_path / "beets.yaml"
    config.write_text(
        f"library: {database}\ndirectory: {tmp_path / 'music'}\n",
        encoding="utf-8",
    )
    try:
        importer.prune_missing_items(config)
    except importer.PruneAborted as error:
        assert (error.missing, error.total) == (2, 2)
    else:
        raise AssertionError("unsafe prune did not abort")

    assert importer.prune_missing_items(config, safety_fraction=1.0) == 2
    assert NativeLibrary(config).items() == []
    assert (tmp_path / "library.db.native-backup").exists()

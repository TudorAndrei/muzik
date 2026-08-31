import json
from pathlib import Path

import pytest

from muzik.core.watchlist import (
    DuplicatePlaylistError,
    InvalidPlaylistUrlError,
    StageRecord,
    StageStatus,
    Watchlist,
    WatchlistFormatError,
    WatchlistItem,
    WatchlistPlaylist,
    WatchlistRepository,
)


def test_missing_watchlist_loads_empty(tmp_path: Path) -> None:
    loaded = WatchlistRepository(tmp_path / "watchlist.json").load()

    assert loaded == Watchlist()


def test_watchlist_save_load_round_trip_keeps_item_state(tmp_path: Path) -> None:
    path = tmp_path / "config" / "watchlist.json"
    repository = WatchlistRepository(path)
    item = WatchlistItem(
        position=3,
        title="Three-hour mix",
        video_id="abcdefghijk",
        video_url="https://www.youtube.com/watch?v=abcdefghijk",
        thumbnail_url="https://img.test/abcdefghijk.jpg",
        last_action="parse_again",
        last_error="old failure",
    )
    item.stages["download"] = StageRecord(
        status=StageStatus.COMPLETE,
        updated_at="2026-08-30T18:00:00",
        path="/music/mix.m4a",
    )
    item.stages["parse"] = StageRecord(
        status=StageStatus.FAILED,
        updated_at="2026-08-30T18:01:00",
        error="No chapters found.",
    )
    saved = Watchlist(
        playlists=[
            WatchlistPlaylist(
                playlist_id="PL123",
                url="https://www.youtube.com/playlist?list=PL123",
                items=[item],
                processed_video_ids=["abcdefghijk"],
                last_checked_at="2026-08-30T18:02:00",
                last_error=None,
            )
        ]
    )

    repository.save(saved)
    loaded = repository.load()

    assert loaded == saved
    assert loaded.playlists[0].items[0].stages["download"].path == "/music/mix.m4a"


def test_add_normalizes_url_and_rejects_duplicate(tmp_path: Path) -> None:
    repository = WatchlistRepository(tmp_path / "watchlist.json")

    added = repository.add(
        " https://www.youtube.com/watch?v=abcdefghijk&list=PL_TEST-1 "
    )

    assert added.playlist_id == "PL_TEST-1"
    assert added.url == "https://www.youtube.com/playlist?list=PL_TEST-1"
    with pytest.raises(DuplicatePlaylistError, match="already in the watchlist"):
        repository.add("https://youtube.com/playlist?list=PL_TEST-1")


def test_add_rejects_non_playlist_url(tmp_path: Path) -> None:
    repository = WatchlistRepository(tmp_path / "watchlist.json")

    with pytest.raises(InvalidPlaylistUrlError, match="YouTube playlist URL"):
        repository.add("https://youtube.com/watch?v=abcdefghijk")


def test_remove_playlist_saves_remaining_entries(tmp_path: Path) -> None:
    repository = WatchlistRepository(tmp_path / "watchlist.json")
    repository.add("https://youtube.com/playlist?list=PL_ONE")
    repository.add("https://youtube.com/playlist?list=PL_TWO")

    assert repository.remove("PL_ONE") is True
    assert repository.remove("missing") is False
    assert [item.playlist_id for item in repository.load().playlists] == ["PL_TWO"]


@pytest.mark.parametrize(
    ("content", "message"),
    [
        ("not-json", "Unable to read watchlist file"),
        ('{"version": 99, "playlists": []}', "Unsupported watchlist version"),
        ('{"version": 1, "playlists": {}}', "must be a list"),
        (
            '{"version": 1, "playlists": [{"playlist_id": "PL", "url": "u", '
            '"items": [{"position": 0, "title": "x"}]}]}',
            "position must be a positive integer",
        ),
    ],
)
def test_invalid_watchlist_is_not_replaced(
    tmp_path: Path,
    content: str,
    message: str,
) -> None:
    path = tmp_path / "watchlist.json"
    path.write_text(content, encoding="utf-8")
    repository = WatchlistRepository(path)

    with pytest.raises(WatchlistFormatError, match=message):
        repository.add("https://youtube.com/playlist?list=PL_NEW")

    assert path.read_text(encoding="utf-8") == content


def test_save_replaces_file_from_same_directory(
    tmp_path: Path,
    monkeypatch,
) -> None:
    path = tmp_path / "config" / "watchlist.json"
    path.parent.mkdir()
    path.write_text('{"version": 1, "playlists": []}\n', encoding="utf-8")
    calls: list[tuple[Path, Path]] = []
    original_replace = Path.replace

    def recording_replace(source: Path, target: Path) -> Path:
        calls.append((source, target))
        return original_replace(source, target)

    monkeypatch.setattr(Path, "replace", recording_replace)
    repository = WatchlistRepository(path)
    repository.save(Watchlist())

    assert calls and calls[0][0].parent == path.parent
    assert calls[0][1] == path
    assert json.loads(path.read_text(encoding="utf-8"))["version"] == 1
    assert list(path.parent.glob(".watchlist.json.*.tmp")) == []

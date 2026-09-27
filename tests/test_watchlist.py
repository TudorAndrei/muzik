from dataclasses import replace
import json
from pathlib import Path

import pytest
from types import SimpleNamespace

from muzik.core.watchlist import (
    DuplicatePlaylistError,
    InvalidPlaylistUrlError,
    StageRecord,
    StageStatus,
    WATCHLIST_VERSION,
    Watchlist,
    WatchlistFormatError,
    WatchlistItem,
    WatchlistPlaylist,
    WatchlistRepository,
    WatchlistSourceKind,
    reconcile_watchlist,
    refresh_watchlist,
)
from muzik.core import cache as cache_mod
from muzik.core.sources.base import ResolvedPlaylist, ResolvedTrack
from muzik.core.sources.spotify_auth import SpotifyAuthError
from muzik.core.sources.youtube import (
    PlaylistLookupError,
    YouTubePlaylist,
    YouTubePlaylistItem,
)
from muzik.core.workflow.service import (
    WorkflowOptions,
    WorkflowRequest,
    WorkflowRunOperations,
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


def test_add_saves_a_spotify_playlist_source(tmp_path: Path) -> None:
    repository = WatchlistRepository(tmp_path / "watchlist.json")

    added = repository.add(
        " https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M?si=1 "
    )

    assert added.playlist_id == "spotify:playlist:37i9dQZF1DXcBWIGoYBM5M"
    assert added.url == "https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M"
    assert added.source_kind is WatchlistSourceKind.SPOTIFY
    assert added.is_refreshable is True
    assert added.state_id == "spotify_37i9dQZF1DXcBWIGoYBM5M"
    saved = repository.load().playlists[0]
    assert saved.source_kind is WatchlistSourceKind.SPOTIFY
    assert saved.url == added.url


def test_add_accepts_the_spotify_liked_songs_collection(tmp_path: Path) -> None:
    repository = WatchlistRepository(tmp_path / "watchlist.json")

    added = repository.add("liked")

    assert added.playlist_id == "spotify:liked"
    assert added.title == "Liked Songs"
    assert added.url == "https://open.spotify.com/collection/tracks"
    assert added.state_id == "spotify_liked"
    with pytest.raises(DuplicatePlaylistError):
        repository.add("https://open.spotify.com/collection/tracks")


def test_rename_gives_a_source_a_name(tmp_path: Path) -> None:
    repository = WatchlistRepository(tmp_path / "watchlist.json")
    repository.add("https://youtube.com/playlist?list=PL_ONE")

    assert repository.rename("PL_ONE", " Jazz albums ") is True
    assert repository.rename("PL_MISSING", "Nothing") is False
    saved = repository.load().playlists[0]
    assert saved.title == "Jazz albums"
    assert saved.display_name == "Jazz albums"


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


def test_version_1_records_migrate_with_quality_not_started_and_data_intact(
    tmp_path: Path,
) -> None:
    """A saved version 1 watchlist has no "quality" key in its stages dict —
    loading it must not reject it, must not touch its existing download/
    parse/split/organize results, and must add "quality" as NOT_STARTED."""
    path = tmp_path / "watchlist.json"
    path.write_text(
        json.dumps(
            {
                "version": 1,
                "playlists": [
                    {
                        "playlist_id": "PL1",
                        "url": "https://www.youtube.com/playlist?list=PL1",
                        "items": [
                            {
                                "position": 1,
                                "title": "Song",
                                "video_id": "abcdefghijk",
                                "stages": {
                                    "download": {
                                        "status": "complete",
                                        "path": "/music/song.flac",
                                    },
                                    "parse": {"status": "complete"},
                                    "split": {"status": "skipped"},
                                    "organize": {"status": "complete"},
                                },
                            }
                        ],
                    }
                ],
            }
        ),
        encoding="utf-8",
    )
    repository = WatchlistRepository(path)

    loaded = repository.load()

    assert loaded.version == WATCHLIST_VERSION
    assert loaded.playlists[0].source_kind is WatchlistSourceKind.YOUTUBE
    assert loaded.playlists[0].title is None
    item = loaded.playlists[0].items[0]
    assert item.stages["quality"].status is StageStatus.NOT_STARTED
    assert item.stages["download"].status is StageStatus.COMPLETE
    assert item.stages["download"].path == "/music/song.flac"
    assert item.stages["parse"].status is StageStatus.COMPLETE
    assert item.stages["split"].status is StageStatus.SKIPPED
    assert item.stages["organize"].status is StageStatus.COMPLETE

    # Saving it back upgrades the file's own version field to the current one.
    repository.save(loaded)
    assert json.loads(path.read_text(encoding="utf-8"))["version"] == WATCHLIST_VERSION


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
    assert json.loads(path.read_text(encoding="utf-8"))["version"] == WATCHLIST_VERSION
    assert list(path.parent.glob(".watchlist.json.*.tmp")) == []


def _playlist_item(
    position: int,
    video_id: str | None,
    title: str | None = None,
) -> YouTubePlaylistItem:
    return YouTubePlaylistItem(
        position=position,
        title=title or f"Video {position}",
        video_id=video_id,
        video_url=(f"https://www.youtube.com/watch?v={video_id}" if video_id else None),
        thumbnail_url=f"https://img.test/{position}.jpg",
    )


def _workflow_operations(
    calls: list[str],
    *,
    fail_ids: set[str] | None = None,
) -> WorkflowRunOperations:
    fail_ids = fail_ids or set()

    def download(url: str, output: Path, archive_file: Path | None) -> bool:
        video_id = url[-11:]
        calls.append(video_id)
        if video_id in fail_ids:
            return False
        output.mkdir(parents=True, exist_ok=True)
        (output / f"Video [{video_id}].m4a").write_bytes(b"audio")
        return True

    return WorkflowRunOperations(
        download_audio=download,
        process_audio=lambda files, split_dirs: None,
        acquire_soulseek=lambda raw: [],
        prepopulate_archive=lambda archive: None,
        get_playlist_video_ids=lambda raw: [],
    )


def _refresh_request(tmp_path: Path) -> WorkflowRequest:
    return WorkflowRequest(
        raw="watchlist",
        output=tmp_path / "downloads",
        splits=tmp_path / "splits",
    )


class CountingRepository(WatchlistRepository):
    def __init__(self, path: Path) -> None:
        super().__init__(path)
        self.save_calls = 0

    def save(self, watchlist: Watchlist) -> None:
        self.save_calls += 1
        super().save(watchlist)


def test_refresh_processes_only_pending_ids_and_saves_each_result(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    repository = CountingRepository(tmp_path / "watchlist.json")
    playlist = repository.add("https://youtube.com/playlist?list=PL_ONE")
    watchlist = repository.load()
    watchlist.playlists[0].processed_video_ids = ["abcdefghijk"]
    repository.save(watchlist)
    repository.save_calls = 0
    calls: list[str] = []

    summary = refresh_watchlist(
        repository,
        _refresh_request(tmp_path),
        WorkflowOptions(no_organize=True),
        operations=_workflow_operations(calls),
        item_loader=lambda url: [
            _playlist_item(1, "abcdefghijk"),
            _playlist_item(2, "lmnopqrstuv"),
        ],
    )

    loaded = repository.load().playlists[0]
    assert playlist.playlist_id == "PL_ONE"
    assert calls == ["lmnopqrstuv"]
    assert loaded.processed_video_ids == ["abcdefghijk", "lmnopqrstuv"]
    assert [item.title for item in loaded.items] == ["Video 1", "Video 2"]
    assert summary.pending_videos == 1
    assert summary.completed_videos == 1
    assert repository.save_calls >= 2

    calls.clear()
    second = refresh_watchlist(
        repository,
        _refresh_request(tmp_path),
        WorkflowOptions(no_organize=True),
        operations=_workflow_operations(calls),
        item_loader=lambda url: [
            _playlist_item(1, "abcdefghijk"),
            _playlist_item(2, "lmnopqrstuv"),
        ],
    )
    assert calls == []
    assert second.pending_videos == 0


def test_refresh_retries_failed_video(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    repository = WatchlistRepository(tmp_path / "watchlist.json")
    repository.add("https://youtube.com/playlist?list=PL_ONE")
    calls: list[str] = []

    def items(url: str) -> list[YouTubePlaylistItem]:
        return [_playlist_item(1, "abcdefghijk")]

    first = refresh_watchlist(
        repository,
        _refresh_request(tmp_path),
        WorkflowOptions(no_organize=True),
        operations=_workflow_operations(calls, fail_ids={"abcdefghijk"}),
        item_loader=items,
    )

    failed_item = repository.load().playlists[0].items[0]
    assert first.failed_videos == 1
    assert failed_item.stages["download"].status is StageStatus.FAILED
    assert repository.load().playlists[0].processed_video_ids == []

    second = refresh_watchlist(
        repository,
        _refresh_request(tmp_path),
        WorkflowOptions(no_organize=True),
        operations=_workflow_operations(calls),
        item_loader=items,
    )
    assert second.completed_videos == 1
    assert calls == ["abcdefghijk", "abcdefghijk"]


def test_refresh_continues_after_playlist_lookup_error(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    repository = WatchlistRepository(tmp_path / "watchlist.json")
    repository.add("https://youtube.com/playlist?list=PL_BAD")
    repository.add("https://youtube.com/playlist?list=PL_GOOD")
    calls: list[str] = []

    def load_items(url: str) -> list[YouTubePlaylistItem]:
        if "PL_BAD" in url:
            raise PlaylistLookupError("Unable to read first playlist.")
        return [_playlist_item(1, "abcdefghijk")]

    summary = refresh_watchlist(
        repository,
        _refresh_request(tmp_path),
        WorkflowOptions(no_organize=True),
        operations=_workflow_operations(calls),
        item_loader=load_items,
    )

    playlists = repository.load().playlists
    assert playlists[0].last_error == "Unable to read first playlist."
    assert playlists[1].processed_video_ids == ["abcdefghijk"]
    assert summary.playlists_checked == 2
    assert calls == ["abcdefghijk"]


def test_refresh_names_the_youtube_source_from_the_playlist(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    repository = WatchlistRepository(tmp_path / "watchlist.json")
    repository.add("https://youtube.com/playlist?list=PL_ONE")
    calls: list[str] = []
    checked: list[str] = []

    def load_playlist(url: str) -> YouTubePlaylist:
        checked.append(url)
        return YouTubePlaylist(
            title="Jazz albums",
            items=[_playlist_item(1, "abcdefghijk")],
        )

    summary = refresh_watchlist(
        repository,
        _refresh_request(tmp_path),
        WorkflowOptions(no_organize=True),
        operations=_workflow_operations(calls),
        item_loader=load_playlist,
    )

    youtube = repository.load().playlists[0]
    assert checked == ["https://www.youtube.com/playlist?list=PL_ONE"]
    assert summary.playlists_checked == 1
    assert youtube.title == "Jazz albums"
    assert calls == ["abcdefghijk"]


def _spotify_playlist(*titles: str) -> ResolvedPlaylist:
    return ResolvedPlaylist(
        title="Liked Songs",
        source="spotify",
        source_id="liked",
        entries=[
            ResolvedTrack(
                title=title,
                artist="Kohsuke Mine",
                album="Sunshower",
                index=index,
                source="spotify",
                source_id=f"spotify:track:track{index}",
                source_url=f"https://open.spotify.com/track/track{index}",
                source_metadata={"image": f"https://i.scdn.co/image/{index}"},
            )
            for index, title in enumerate(titles, start=1)
        ],
    )


def test_refresh_syncs_a_spotify_source_and_acquires_pending_tracks(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    repository = WatchlistRepository(tmp_path / "watchlist.json")
    repository.add("liked")
    acquired: list[str] = []

    def acquire_track(track: ResolvedTrack) -> list[Path]:
        acquired.append(track.title)
        if track.title == "Missing":
            return []
        path = tmp_path / f"{track.title}.flac"
        path.write_bytes(b"audio")
        return [path]

    operations = _workflow_operations([])
    operations = replace(operations, acquire_soulseek_track=acquire_track)

    summary = refresh_watchlist(
        repository,
        _refresh_request(tmp_path),
        WorkflowOptions(audio_source="soulseek"),
        operations=operations,
        spotify_loader=lambda uri: _spotify_playlist("Sunshower", "Missing"),
    )

    saved = repository.load().playlists[0]
    assert acquired == ["Sunshower", "Missing"]
    assert summary.pending_videos == 2
    assert summary.completed_videos == 1
    assert summary.failed_videos == 1
    assert saved.title == "Liked Songs"
    assert [item.title for item in saved.items] == [
        "Kohsuke Mine - Sunshower",
        "Kohsuke Mine - Missing",
    ]
    done, failed = saved.items
    assert done.kind == "spotify"
    assert done.video_id == "track1"
    assert done.entry_id == "spotify:track:track1#0"
    assert done.stages["download"].status is StageStatus.COMPLETE
    assert done.stages["split"].status is StageStatus.SKIPPED
    assert done.stages["organize"].status is StageStatus.COMPLETE
    assert saved.processed_video_ids == ["spotify:track:track1#0"]
    assert failed.stages["download"].status is StageStatus.FAILED
    assert failed.last_error is not None


def test_refresh_keeps_organized_spotify_tracks_and_adds_new_ones(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    repository = WatchlistRepository(tmp_path / "watchlist.json")
    repository.add("liked")
    acquired: list[str] = []

    def acquire_track(track: ResolvedTrack) -> list[Path]:
        acquired.append(track.title)
        path = tmp_path / f"{track.title}.flac"
        path.write_bytes(b"audio")
        return [path]

    operations = replace(_workflow_operations([]), acquire_soulseek_track=acquire_track)
    options = WorkflowOptions(audio_source="soulseek")
    refresh_watchlist(
        repository,
        _refresh_request(tmp_path),
        options,
        operations=operations,
        spotify_loader=lambda uri: _spotify_playlist("Sunshower"),
    )
    refresh_watchlist(
        repository,
        _refresh_request(tmp_path),
        options,
        operations=operations,
        spotify_loader=lambda uri: _spotify_playlist("Sunshower", "Scenery"),
    )

    saved = repository.load().playlists[0]
    assert acquired == ["Sunshower", "Scenery"]
    assert len(saved.items) == 2
    assert saved.processed_video_ids == [
        "spotify:track:track1#0",
        "spotify:track:track2#0",
    ]


def test_refresh_reports_a_spotify_error_without_stopping_youtube(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    repository = WatchlistRepository(tmp_path / "watchlist.json")
    repository.add("liked")
    repository.add("https://youtube.com/playlist?list=PL_ONE")
    calls: list[str] = []

    def broken_loader(uri: str) -> ResolvedPlaylist:
        raise SpotifyAuthError("muzik is not connected to Spotify.")

    summary = refresh_watchlist(
        repository,
        _refresh_request(tmp_path),
        WorkflowOptions(no_organize=True),
        operations=_workflow_operations(calls),
        item_loader=lambda url: [_playlist_item(1, "abcdefghijk")],
        spotify_loader=broken_loader,
    )

    spotify, youtube = repository.load().playlists
    assert spotify.last_error == "muzik is not connected to Spotify."
    assert summary.playlist_errors == 1
    assert youtube.processed_video_ids == ["abcdefghijk"]
    assert calls == ["abcdefghijk"]


def test_refresh_replaces_current_items_but_keeps_processed_ids(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    repository = WatchlistRepository(tmp_path / "watchlist.json")
    repository.add("https://youtube.com/playlist?list=PL_ONE")
    watchlist = repository.load()
    watchlist.playlists[0].items = [
        WatchlistItem(position=1, title="Removed", video_id="abcdefghijk")
    ]
    watchlist.playlists[0].processed_video_ids = ["abcdefghijk"]
    repository.save(watchlist)

    refresh_watchlist(
        repository,
        _refresh_request(tmp_path),
        WorkflowOptions(no_organize=True),
        operations=_workflow_operations([]),
        item_loader=lambda url: [_playlist_item(1, "lmnopqrstuv", "Current")],
    )

    loaded = repository.load().playlists[0]
    assert [item.video_id for item in loaded.items] == ["lmnopqrstuv"]
    assert loaded.processed_video_ids == ["abcdefghijk", "lmnopqrstuv"]


def test_refresh_keeps_unavailable_item_visible_without_work(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    repository = WatchlistRepository(tmp_path / "watchlist.json")
    repository.add("https://youtube.com/playlist?list=PL_ONE")
    calls: list[str] = []

    summary = refresh_watchlist(
        repository,
        _refresh_request(tmp_path),
        WorkflowOptions(),
        operations=_workflow_operations(calls),
        item_loader=lambda url: [_playlist_item(1, None, "Private video")],
    )

    assert repository.load().playlists[0].items[0].title == "Private video"
    assert calls == []
    assert summary.pending_videos == 0


def test_reconcile_watchlist_reads_playlist_state_and_download_folder(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    monkeypatch.setattr(
        "muzik.core.watchlist.open_library_for_reads", lambda config_path=None: None
    )
    cache_mod.set_json(
        "playlist_PL_ONE",
        {
            "playlist_id": "PL_ONE",
            "videos": {
                "abcdefghijk": {"status": "organized"},
                "lmnopqrstuv": {
                    "status": "split",
                    "audio_file": str(tmp_path / "source.m4a"),
                    "split_dir": str(tmp_path / "splits" / "Mix"),
                },
            },
        },
    )
    downloads = tmp_path / "downloads"
    downloads.mkdir()
    local = downloads / "Local [zyxwvutsrqp].m4a"
    local.write_bytes(b"audio")
    watchlist = Watchlist(
        playlists=[
            WatchlistPlaylist(
                playlist_id="PL_ONE",
                url="https://youtube.com/playlist?list=PL_ONE",
                items=[
                    WatchlistItem(1, "Organized", "abcdefghijk"),
                    WatchlistItem(2, "Split", "lmnopqrstuv"),
                    WatchlistItem(3, "Downloaded", "zyxwvutsrqp"),
                ],
            )
        ]
    )

    reconcile_watchlist(
        watchlist,
        request=_refresh_request(tmp_path),
        options=WorkflowOptions(),
    )

    organized, split, downloaded = watchlist.playlists[0].items
    assert organized.stages["organize"].status is StageStatus.COMPLETE
    assert split.stages["split"].status is StageStatus.COMPLETE
    assert split.stages["split"].path == str(tmp_path / "splits" / "Mix")
    assert downloaded.stages["download"].path == str(local.resolve())


def test_reconcile_watchlist_repairs_false_processed_beets_skip(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    monkeypatch.setattr(
        "muzik.core.watchlist.open_library_for_reads", lambda config_path=None: None
    )
    video_id = "-ON_sl7ZGdk"
    audio = tmp_path / "downloads" / f"Terrace Brothers + Asa [1997] [{video_id}].opus"
    split_dir = tmp_path / "splits" / audio.stem
    split_dir.mkdir(parents=True)
    (split_dir / "01-opening.opus").write_bytes(b"audio")
    cache_mod.set_json(
        "playlist_PL_ONE",
        {
            "playlist_id": "PL_ONE",
            "videos": {
                video_id: {
                    "status": "organized",
                    "audio_file": str(audio),
                }
            },
        },
    )
    item = WatchlistItem(1, "Terrace Brothers + Asa [1997]", video_id)
    watchlist = Watchlist(
        playlists=[
            WatchlistPlaylist(
                playlist_id="PL_ONE",
                url="https://youtube.com/playlist?list=PL_ONE",
                items=[item],
                processed_video_ids=[video_id],
            )
        ]
    )

    reconcile_watchlist(
        watchlist,
        request=_refresh_request(tmp_path),
        options=WorkflowOptions(),
    )

    assert watchlist.playlists[0].processed_video_ids == []
    assert item.stages["split"].status is StageStatus.COMPLETE
    assert item.stages["split"].path == str(split_dir)
    assert item.stages["organize"].status is StageStatus.FAILED
    assert (
        item.last_error == "The music library did not import this item. Select Retry."
    )


class FakeLibrary:
    def __init__(self, music: Path) -> None:
        self.directory = str(music)
        self._items: list[SimpleNamespace] = []
        self._albums: list[SimpleNamespace] = []

    def items(self, query=None):
        return list(self._items)

    def albums(self, query=""):
        return list(self._albums)


def _library(tmp_path: Path) -> FakeLibrary:
    music = tmp_path / "music"
    music.mkdir()
    return FakeLibrary(music)


def _add_album(lib: FakeLibrary, path: Path, *, artist: str, album: str) -> None:
    item = SimpleNamespace(
        path=str(path).encode(),
        title="Track 1",
        artist=artist,
        album=album,
        albumartist=artist,
        get=lambda key: None,
    )
    lib._items.append(item)
    lib._albums.append(
        SimpleNamespace(
            albumartist=artist,
            album=album,
            items=lambda: [item],
        )
    )


def test_reconcile_watchlist_finds_an_already_organized_album_in_beets(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    lib = _library(tmp_path)
    track = Path(lib.directory) / "Etnobotanika" / "01 Track 1.mp3"
    track.parent.mkdir(parents=True)
    track.write_bytes(b"audio")
    _add_album(lib, track, artist="Etnobotanika", album="Kosmobotanika")
    monkeypatch.setattr(
        "muzik.core.watchlist.open_library_for_reads", lambda config_path=None: lib
    )
    video_id = "gZUPDL3RBYs"
    watchlist = Watchlist(
        playlists=[
            WatchlistPlaylist(
                playlist_id="PL_ONE",
                url="https://youtube.com/playlist?list=PL_ONE",
                items=[
                    WatchlistItem(
                        1,
                        "Etnobotanika - Kosmobotanika (Full Album)",
                        video_id,
                    )
                ],
            )
        ]
    )

    reconcile_watchlist(
        watchlist,
        request=_refresh_request(tmp_path),
        options=WorkflowOptions(),
    )

    item = watchlist.playlists[0].items[0]
    assert item.stages["download"].status is StageStatus.COMPLETE
    assert item.stages["download"].path == str(track)
    assert item.stages["parse"].status is StageStatus.COMPLETE
    assert item.stages["split"].status is StageStatus.SKIPPED
    assert item.stages["organize"].status is StageStatus.COMPLETE
    assert watchlist.playlists[0].processed_video_ids == [video_id]


def test_reconcile_watchlist_finds_an_album_by_exact_source_id(
    tmp_path: Path,
    monkeypatch,
) -> None:
    # The title deliberately does not match the beets album at all — this
    # only passes if the exact source-id match is tried, not title parsing.
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    lib = _library(tmp_path)
    track = Path(lib.directory) / "Etnobotanika" / "01 Track 1.mp3"
    track.parent.mkdir(parents=True)
    track.write_bytes(b"audio")
    item = SimpleNamespace(
        path=str(track).encode(),
        title="Track 1",
        artist="Etnobotanika",
        album="Kosmobotanika",
        albumartist="Etnobotanika",
        get=lambda key: "gZUPDL3RBYs" if key == "muzik_source_id" else None,
    )
    lib._items.append(item)
    monkeypatch.setattr(
        "muzik.core.watchlist.open_library_for_reads", lambda config_path=None: lib
    )
    video_id = "gZUPDL3RBYs"
    watchlist = Watchlist(
        playlists=[
            WatchlistPlaylist(
                playlist_id="PL_ONE",
                url="https://youtube.com/playlist?list=PL_ONE",
                items=[WatchlistItem(1, "A totally unrelated video title", video_id)],
            )
        ]
    )

    reconcile_watchlist(
        watchlist,
        request=_refresh_request(tmp_path),
        options=WorkflowOptions(),
    )

    item = watchlist.playlists[0].items[0]
    assert item.stages["download"].status is StageStatus.COMPLETE
    assert item.stages["download"].path == str(track)
    assert item.stages["organize"].status is StageStatus.COMPLETE
    assert watchlist.playlists[0].processed_video_ids == [video_id]


def test_reconcile_watchlist_ignores_an_unrelated_library(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    lib = _library(tmp_path)
    track = Path(lib.directory) / "Other Artist" / "01 Track 1.mp3"
    track.parent.mkdir(parents=True)
    track.write_bytes(b"audio")
    _add_album(lib, track, artist="Other Artist", album="Other Album")
    monkeypatch.setattr(
        "muzik.core.watchlist.open_library_for_reads", lambda config_path=None: lib
    )
    video_id = "gZUPDL3RBYs"
    watchlist = Watchlist(
        playlists=[
            WatchlistPlaylist(
                playlist_id="PL_ONE",
                url="https://youtube.com/playlist?list=PL_ONE",
                items=[
                    WatchlistItem(
                        1,
                        "Etnobotanika - Kosmobotanika (Full Album)",
                        video_id,
                    )
                ],
            )
        ]
    )

    reconcile_watchlist(
        watchlist,
        request=_refresh_request(tmp_path),
        options=WorkflowOptions(),
    )

    item = watchlist.playlists[0].items[0]
    assert item.stages["download"].status is StageStatus.NOT_STARTED
    assert watchlist.playlists[0].processed_video_ids == []

"""Process protocol checks for the native GUI service."""

from io import StringIO
import json
from pathlib import Path
from queue import Queue
from threading import Event, Thread
import time
from types import SimpleNamespace
from typing import TextIO, cast

import pytest

from muzik.config import DEFAULT_DOWNLOAD_DIR, DEFAULT_SPLITS_DIR
from muzik.core.quality import QualityPolicy
from muzik.core.watchlist import (
    StageStatus,
    Watchlist,
    WatchlistItem,
    WatchlistPlaylist,
    WatchlistRepository,
)
from muzik.core.sources.spotify_api import SpotifyPlaylistRef
from muzik.core.sources.base import Candidate, ResolvedTrack
from muzik.core.thumbnails import ThumbnailResult
from muzik.core.workflow.item_actions import ItemAction, ItemActionOperations
from muzik.core.workflow.cancellation import CancellationToken
from muzik.core.workflow.decisions import WorkflowDecisionError
from muzik.core.workflow.events import MessageEvent
from muzik.core.workflow.service import (
    AudioFallback,
    AudioSource,
    MetadataSource,
    WorkflowOptions,
    WorkflowRequest,
)
from muzik.core.workflow.service import QualityUpgradeResult
from muzik.native_gui.server import (
    NativeGuiServer,
    _WorkflowDecisions,
    _request_options,
    _watchlist_data,
)


def _records(writer: StringIO) -> list[dict]:
    return [json.loads(line) for line in writer.getvalue().splitlines()]


class _RequestStream:
    def __init__(self) -> None:
        self.lines: Queue[str | None] = Queue()

    def __iter__(self):
        while (line := self.lines.get()) is not None:
            yield line

    def send(self, request: dict) -> None:
        self.lines.put(json.dumps(request) + "\n")

    def close(self) -> None:
        self.lines.put(None)


def _wait_for_record(server: NativeGuiServer, writer: StringIO, predicate):
    deadline = time.monotonic() + 2
    while time.monotonic() < deadline:
        with server._write_lock:
            records = _records(writer)
        for record in records:
            if predicate(record):
                return record
        time.sleep(0.01)
    raise AssertionError(f"No matching response in {records!r}")


def test_request_options_maps_every_workflow_field(tmp_path: Path) -> None:
    params = {
        "raw": "  https://example.test/album  ",
        "output": str(tmp_path / "downloads"),
        "splits": str(tmp_path / "splits"),
        "review": True,
        "no_split": True,
        "no_organize": True,
        "import_": True,
        "tag_only": True,
        "dry_run": True,
        "jobs": 3,
        "config": str(tmp_path / "beets.yaml"),
        "keep_source": True,
        "force": True,
        "metadata_source": "musicbrainz",
        "audio_source": "soulseek",
        "prefer": "flac",
        "fallback": "none",
        "interactive": False,
        "quality_policy": "ask",
        "min_bitrate": 192,
    }

    request, options = _request_options(params)

    assert request == WorkflowRequest(
        "https://example.test/album", tmp_path / "downloads", tmp_path / "splits"
    )
    assert options == WorkflowOptions(
        review=True,
        no_split=True,
        no_organize=True,
        import_=True,
        tag_only=True,
        dry_run=True,
        jobs=3,
        config=tmp_path / "beets.yaml",
        keep_source=True,
        force=True,
        metadata_source=MetadataSource.MUSICBRAINZ,
        audio_source=AudioSource.SOULSEEK,
        prefer="flac",
        fallback=AudioFallback.NONE,
        interactive=False,
        quality_policy=QualityPolicy.ASK,
        min_bitrate=192,
    )


def test_request_options_uses_workflow_defaults() -> None:
    request, options = _request_options({"raw": " local.flac ", "config": ""})

    assert request == WorkflowRequest(
        "local.flac", DEFAULT_DOWNLOAD_DIR, DEFAULT_SPLITS_DIR
    )
    assert options == WorkflowOptions()


def test_process_round_trip_and_watchlist_storage(tmp_path) -> None:
    reader = StringIO(
        "broken json\n"
        '{"id":"a","command":"hello","params":{}}\n'
        '{"id":"b","command":"watchlist.add","params":{"url":"https://www.youtube.com/playlist?list=PL123456789012345"}}\n'
        '{"id":"c","command":"watchlist.load","params":{}}\n'
    )
    writer = StringIO()
    server = NativeGuiServer(
        reader, writer, repository=WatchlistRepository(tmp_path / "watchlist.json")
    )
    server.serve()
    records = [record for record in _records(writer) if record["type"] == "response"]
    assert [record["id"] for record in records] == [None, "a", "b", "c"]
    assert records[0]["ok"] is False
    assert all(record["ok"] for record in records[1:])
    assert records[1]["result"]["protocol_version"] == 1
    assert (
        records[3]["result"]["watchlist"]["playlists"][0]["playlist_id"]
        == "PL123456789012345"
    )
    renamed = server.dispatch(
        "watchlist.rename", {"playlist_id": "PL123456789012345", "title": "Mine"}
    )
    assert renamed["renamed"] is True
    assert renamed["watchlist"]["playlists"][0]["title"] == "Mine"
    removed = server.dispatch("watchlist.remove", {"playlist_id": "PL123456789012345"})
    assert removed["removed"] is True
    assert server.repository.load().playlists == []


def test_worker_emits_events_and_terminal_result(monkeypatch, tmp_path) -> None:
    writer = StringIO()
    server = NativeGuiServer(
        StringIO(), writer, repository=WatchlistRepository(tmp_path / "watchlist.json")
    )
    monkeypatch.setattr(
        "muzik.native_gui.server.build_workflow_operations", lambda **kwargs: object()
    )

    def run(request, options, *, operations, events, cancellation):
        events.emit(MessageEvent("Working"))

    monkeypatch.setattr("muzik.native_gui.server.run_workflow", run)
    job_id = server.dispatch("workflow.start", {"raw": "example"})["job_id"]
    assert server._job is not None
    server._job.join(timeout=2)
    records = _records(writer)
    assert records[0]["event"] == "job.event"
    assert records[0]["data"]["event"] == "message"
    assert records[0]["data"]["data"]["message"] == "Working"
    assert records[1] == {
        "type": "event",
        "event": "job.completed",
        "data": {"job_id": job_id, "result": {}},
    }


def test_decision_reply_unblocks_worker(tmp_path) -> None:
    writer = StringIO()
    server = NativeGuiServer(
        StringIO(), writer, repository=WatchlistRepository(tmp_path / "watchlist.json")
    )
    answer: list[object] = []
    worker = Thread(
        target=lambda: answer.append(
            server._request_decision(
                "job", "chapter_review", {"chapters": []}, CancellationToken()
            )
        )
    )
    worker.start()
    deadline = time.monotonic() + 2
    while not writer.getvalue() and time.monotonic() < deadline:
        time.sleep(0.01)
    decision_id = _records(writer)[0]["data"]["decision_id"]
    server.dispatch("decision.reply", {"decision_id": decision_id, "value": "accept"})
    worker.join(timeout=2)
    assert not worker.is_alive()
    assert answer == ["accept"]


def test_skipping_soulseek_candidates_reports_clear_error(
    monkeypatch, tmp_path
) -> None:
    server = NativeGuiServer(
        StringIO(),
        StringIO(),
        repository=WatchlistRepository(tmp_path / "watchlist.json"),
    )
    monkeypatch.setattr(server, "_request_decision", lambda *_args: None)
    decisions = _WorkflowDecisions(server, "job", True, CancellationToken())
    with pytest.raises(WorkflowDecisionError, match="No Soulseek candidate selected"):
        decisions.choose_soulseek_candidate([cast(Candidate, object())])


def test_cancel_pending_decision_emits_cancelled_event(monkeypatch, tmp_path) -> None:
    writer = StringIO()
    server = NativeGuiServer(
        StringIO(), writer, repository=WatchlistRepository(tmp_path / "watchlist.json")
    )
    monkeypatch.setattr(
        "muzik.native_gui.server.build_workflow_operations",
        lambda **kwargs: SimpleNamespace(decisions=kwargs["decisions"]),
    )

    def run(request, options, *, operations, events, cancellation):
        operations.decisions.confirm_chapters(tmp_path / "audio.m4a", [])

    monkeypatch.setattr("muzik.native_gui.server.run_workflow", run)
    job_id = server.dispatch("workflow.start", {"raw": "example"})["job_id"]
    deadline = time.monotonic() + 2
    while time.monotonic() < deadline:
        with server._write_lock:
            records = _records(writer)
        if records:
            break
        time.sleep(0.01)
    assert records[0]["event"] == "decision.request"
    decision_id = records[0]["data"]["decision_id"]
    assert server.dispatch("job.cancel", {"job_id": job_id})["cancel_requested"]
    assert server._job is not None
    server._job.join(timeout=2)
    assert not server._job.is_alive()
    assert _records(writer)[-1] == {
        "type": "event",
        "event": "job.cancelled",
        "data": {"job_id": job_id},
    }
    with pytest.raises(ValueError, match="not pending"):
        server.dispatch(
            "decision.reply", {"decision_id": decision_id, "value": "accept"}
        )


def test_watchlist_card_data_and_active_job_guard(monkeypatch, tmp_path) -> None:
    watchlist = Watchlist(
        playlists=[
            WatchlistPlaylist(
                playlist_id="PL123456789012345",
                url="https://www.youtube.com/playlist?list=PL123456789012345",
                items=[
                    WatchlistItem(
                        position=1,
                        title="Track",
                        video_id="dQw4w9WgXcQ",
                        video_url="https://www.youtube.com/watch?v=dQw4w9WgXcQ",
                    )
                ],
            )
        ]
    )
    card = _watchlist_data(watchlist)["playlists"][0]["items"][0]
    assert card["summary"] == "Pending"
    assert card["thumbnail_path"] is None
    assert card["primary_action"] == {"action": "run", "label": "Run"}
    assert card["actions"]["run"] == {"enabled": True, "reason": None}
    assert card["actions"]["parse_again"] == {
        "enabled": False,
        "reason": "Download this video before you run this command.",
    }
    watchlist.playlists[0].items[0].stages["download"].status = StageStatus.FAILED
    assert _watchlist_data(watchlist)["playlists"][0]["items"][0]["primary_action"] == {
        "action": "retry",
        "label": "Retry",
    }

    server = NativeGuiServer(
        StringIO(),
        StringIO(),
        repository=WatchlistRepository(tmp_path / "watchlist.json"),
    )
    started = Event()
    release = Event()
    monkeypatch.setattr(
        "muzik.native_gui.server.build_workflow_operations", lambda **kwargs: object()
    )

    def run(request, options, *, operations, events, cancellation):
        started.set()
        release.wait(2)

    monkeypatch.setattr("muzik.native_gui.server.run_workflow", run)
    server.dispatch("workflow.start", {"raw": "example"})
    assert started.wait(2)
    try:
        with pytest.raises(RuntimeError, match="job is already active"):
            server.dispatch(
                "watchlist.add",
                {"url": "https://www.youtube.com/playlist?list=PL123456789012345"},
            )
    finally:
        release.set()
        assert server._job is not None
        server._job.join(timeout=2)


def test_spotify_status_and_playlists_protocol(monkeypatch, tmp_path) -> None:
    monkeypatch.setattr(
        "muzik.native_gui.server.get_spotify_settings",
        lambda: {"client_id": "app-id", "redirect_port": "8888"},
    )
    monkeypatch.setattr(
        "muzik.native_gui.server.TokenStore",
        lambda: SimpleNamespace(load=lambda: object()),
    )

    class Client:
        def account_name(self) -> str:
            return "Listener"

        def list_playlists(self) -> list[SpotifyPlaylistRef]:
            return [
                SpotifyPlaylistRef(
                    uri="spotify:liked", name="Liked Songs", owner="you", total=4
                )
            ]

    monkeypatch.setattr("muzik.native_gui.server.SpotifyClient", Client)
    reader = StringIO(
        '{"id":"s","command":"spotify.status","params":{}}\n'
        '{"id":"p","command":"spotify.playlists","params":{}}\n'
    )
    writer = StringIO()
    NativeGuiServer(
        reader, writer, repository=WatchlistRepository(tmp_path / "watchlist.json")
    ).serve()
    records = {record["id"]: record for record in _records(writer)}
    status, playlists = records["s"], records["p"]
    assert status["ok"] is True
    assert status["result"] == {
        "client_id": "app-id",
        "redirect_uri": "http://127.0.0.1:8888/callback",
        "connected": True,
        "account_name": "Listener",
    }
    assert playlists["result"]["playlists"] == [
        {
            "uri": "spotify:liked",
            "name": "Liked Songs",
            "owner": "you",
            "total": 4,
            "image_url": None,
        }
    ]


def test_spotify_item_action_uses_source_and_entry_key(monkeypatch, tmp_path) -> None:
    track = ResolvedTrack(
        title="Track",
        source="spotify",
        source_id="spotify:track:track-7",
        source_url="https://open.spotify.com/track/track-7",
    )
    item = WatchlistItem.from_track(track, "entry-7", position=1)
    playlist = WatchlistPlaylist(
        playlist_id="spotify:playlist:source-42",
        url="https://open.spotify.com/playlist/source-42",
        kind="spotify",
        items=[item],
    )
    repository = WatchlistRepository(tmp_path / "watchlist.json")
    repository.save(Watchlist(playlists=[playlist]))
    acquired: list[str] = []

    def run_track(track, source_id, options, cancellation):
        acquired.append(source_id)

    def unexpected_quality(path, options, cancellation) -> QualityUpgradeResult:
        raise AssertionError("A Spotify run cannot check YouTube quality.")

    monkeypatch.setattr(
        "muzik.native_gui.server.build_item_action_operations",
        lambda **kwargs: ItemActionOperations(
            run_workflow=lambda *args: None,
            parse_chapters=lambda *args: tmp_path,
            check_quality=unexpected_quality,
            run_track=run_track,
        ),
    )
    writer = StringIO()
    server = NativeGuiServer(StringIO(), writer, repository=repository)
    result = server._run_item_action(
        {
            "playlist_id": playlist.playlist_id,
            "position": 1,
            "video_id": "track-7",
            "action": ItemAction.RUN.value,
        },
        WorkflowRequest("", tmp_path, tmp_path / "splits"),
        WorkflowOptions(),
        None,
        None,
        None,
        None,
        CancellationToken(),
    )
    assert acquired == ["source-42"]
    assert result["watchlist"]["playlists"][0]["processed_video_ids"] == ["entry-7"]
    assert repository.load().playlists[0].processed_video_ids == ["entry-7"]


def test_watchlist_load_returns_before_reconcile_and_keeps_newer_write(
    monkeypatch, tmp_path
) -> None:
    repository = WatchlistRepository(tmp_path / "watchlist.json")
    repository.add("https://www.youtube.com/playlist?list=PL123456789012345")
    entered = Event()
    release = Event()
    calls: list[str] = []

    def reconcile(watchlist, *, request, options):
        calls.append(watchlist.playlists[0].title or "old")
        entered.set()
        release.wait(2)

    monkeypatch.setattr("muzik.native_gui.server.reconcile_watchlist", reconcile)
    writer = StringIO()
    server = NativeGuiServer(StringIO(), writer, repository=repository)
    start = time.monotonic()
    loaded = server.dispatch("watchlist.load", {})
    assert time.monotonic() - start < 0.5
    assert loaded["watchlist"]["playlists"][0]["title"] is None
    assert entered.wait(2)
    repository.rename("PL123456789012345", "New name")
    release.set()
    assert server._reconcile_worker is not None
    server._reconcile_worker.join(timeout=2)
    assert not server._reconcile_worker.is_alive()
    assert repository.load().playlists[0].title == "New name"
    assert calls == ["old", "New name"]
    assert _records(writer)[-1]["event"] == "watchlist.updated"


def test_watchlist_response_precedes_reconcile_event(monkeypatch, tmp_path) -> None:
    repository = WatchlistRepository(tmp_path / "watchlist.json")
    repository.add("https://www.youtube.com/playlist?list=PL123456789012345")

    def reconcile(watchlist, *, request, options):
        watchlist.playlists[0].title = "Checked title"

    monkeypatch.setattr("muzik.native_gui.server.reconcile_watchlist", reconcile)
    writer = StringIO()
    server = NativeGuiServer(
        StringIO('{"id":"load","command":"watchlist.load","params":{}}\n'),
        writer,
        repository=repository,
    )
    server.serve()
    records = _records(writer)
    assert records[0]["id"] == "load"
    assert records[0]["result"]["watchlist"]["playlists"][0]["title"] is None
    assert records[1]["event"] == "watchlist.updated"
    assert records[1]["data"]["watchlist"]["playlists"][0]["title"] == "Checked title"


def test_slow_read_does_not_delay_decision_reply(monkeypatch, tmp_path) -> None:
    entered = Event()
    release = Event()

    def slow_check():
        entered.set()
        assert release.wait(2)
        return []

    monkeypatch.setattr("muzik.native_gui.server.check_services", slow_check)
    reader = _RequestStream()
    writer = StringIO()
    server = NativeGuiServer(
        cast(TextIO, reader),
        writer,
        repository=WatchlistRepository(tmp_path / "watchlist.json"),
    )
    serving = Thread(target=server.serve)
    serving.start()
    answer: list[object] = []
    decision = Thread(
        target=lambda: answer.append(
            server._request_decision(
                "job", "chapter_review", {"chapters": []}, CancellationToken()
            )
        )
    )
    decision.start()
    try:
        request = _wait_for_record(
            server,
            writer,
            lambda record: record.get("event") == "decision.request",
        )
        reader.send({"id": "slow", "command": "services.check", "params": {}})
        assert entered.wait(2)
        reader.send(
            {
                "id": "reply",
                "command": "decision.reply",
                "params": {
                    "decision_id": request["data"]["decision_id"],
                    "value": "accept",
                },
            }
        )
        response = _wait_for_record(
            server, writer, lambda record: record.get("id") == "reply"
        )
        assert response["ok"] is True
        decision.join(timeout=2)
        assert answer == ["accept"]
        assert not any(record.get("id") == "slow" for record in _records(writer))
    finally:
        release.set()
        reader.close()
        decision.join(timeout=2)
        serving.join(timeout=2)
    assert not serving.is_alive()
    assert any(record.get("id") == "slow" for record in _records(writer))


def test_slow_read_does_not_delay_job_cancel(monkeypatch, tmp_path) -> None:
    entered = Event()
    release = Event()

    def slow_scan(output):
        entered.set()
        assert release.wait(2)
        return []

    monkeypatch.setattr("muzik.native_gui.server.scan_downloads", slow_scan)
    monkeypatch.setattr(
        "muzik.native_gui.server.build_workflow_operations",
        lambda **kwargs: SimpleNamespace(decisions=kwargs["decisions"]),
    )

    def run(request, options, *, operations, events, cancellation):
        operations.decisions.confirm_chapters(tmp_path / "audio.m4a", [])

    monkeypatch.setattr("muzik.native_gui.server.run_workflow", run)
    reader = _RequestStream()
    writer = StringIO()
    server = NativeGuiServer(
        cast(TextIO, reader),
        writer,
        repository=WatchlistRepository(tmp_path / "watchlist.json"),
    )
    serving = Thread(target=server.serve)
    serving.start()
    try:
        reader.send(
            {"id": "start", "command": "workflow.start", "params": {"raw": "example"}}
        )
        response = _wait_for_record(
            server, writer, lambda record: record.get("id") == "start"
        )
        job_id = response["result"]["job_id"]
        _wait_for_record(
            server, writer, lambda record: record.get("event") == "decision.request"
        )
        reader.send(
            {
                "id": "scan",
                "command": "library.scan",
                "params": {"output": str(tmp_path)},
            }
        )
        assert entered.wait(2)
        reader.send(
            {"id": "cancel", "command": "job.cancel", "params": {"job_id": job_id}}
        )
        cancelled = _wait_for_record(
            server, writer, lambda record: record.get("id") == "cancel"
        )
        assert cancelled["ok"] is True
        _wait_for_record(
            server, writer, lambda record: record.get("event") == "job.cancelled"
        )
        with server._write_lock:
            assert not any(record.get("id") == "scan" for record in _records(writer))
    finally:
        release.set()
        reader.close()
        serving.join(timeout=2)
    assert not serving.is_alive()
    assert any(record.get("id") == "scan" for record in _records(writer))


@pytest.mark.parametrize(
    "command",
    ["library.scan", "services.check", "spotify.status", "spotify.playlists"],
)
def test_read_response_keeps_its_id_when_later_command_finishes_first(
    monkeypatch, tmp_path, command
) -> None:
    entered = Event()
    release = Event()
    reader = _RequestStream()
    writer = StringIO()
    server = NativeGuiServer(
        cast(TextIO, reader),
        writer,
        repository=WatchlistRepository(tmp_path / "watchlist.json"),
    )
    dispatch = server.dispatch

    def delayed(request_command, params):
        if request_command == command:
            entered.set()
            assert release.wait(5)
            return {"command": request_command}
        return dispatch(request_command, params)

    monkeypatch.setattr(server, "dispatch", delayed)
    serving = Thread(target=server.serve)
    serving.start()
    try:
        reader.send({"id": "slow", "command": command, "params": {}})
        assert entered.wait(2)
        reader.send({"id": "fast", "command": "hello", "params": {}})
        fast = _wait_for_record(
            server, writer, lambda record: record.get("id") == "fast"
        )
        assert fast["ok"] is True
        assert not any(record.get("id") == "slow" for record in _records(writer))
    finally:
        release.set()
        reader.close()
        serving.join(timeout=2)
    assert not serving.is_alive()
    slow = next(record for record in _records(writer) if record.get("id") == "slow")
    assert slow["result"] == {"command": command}


def test_thumbnail_command_fetches_only_requested_cards(monkeypatch, tmp_path) -> None:
    repository = WatchlistRepository(tmp_path / "watchlist.json")
    repository.save(
        Watchlist(
            playlists=[
                WatchlistPlaylist(
                    playlist_id="PL123456789012345",
                    url="https://www.youtube.com/playlist?list=PL123456789012345",
                    items=[
                        WatchlistItem(
                            position=position,
                            title=f"Track {position}",
                            video_id=video_id,
                            thumbnail_url=f"https://i.ytimg.com/vi/{video_id}/hqdefault.jpg",
                        )
                        for position, video_id in enumerate(
                            ("dQw4w9WgXcQ", "oHg5SJYRHA0"), start=1
                        )
                    ],
                )
            ]
        )
    )
    fetched: list[str] = []

    async def cache(requests):
        fetched.extend(request.video_id for request in requests)
        return [
            ThumbnailResult(video_id=request.video_id, path=tmp_path / "image.jpg")
            for request in requests
        ]

    monkeypatch.setattr("muzik.native_gui.server.cache_thumbnails", cache)
    monkeypatch.setattr("muzik.native_gui.server.cached_thumbnail_path", lambda _: None)
    writer = StringIO()
    server = NativeGuiServer(StringIO(), writer, repository=repository)
    assert server.dispatch("thumbnails.cache", {"video_ids": ["oHg5SJYRHA0"]}) == {
        "queued": 1
    }
    server._thumbnail_pool.shutdown(wait=True)
    assert fetched == ["oHg5SJYRHA0"]
    assert _records(writer) == [
        {
            "type": "event",
            "event": "thumbnails.updated",
            "data": {
                "thumbnails": [
                    {
                        "video_id": "oHg5SJYRHA0",
                        "path": str(tmp_path / "image.jpg"),
                        "error": None,
                    }
                ]
            },
        }
    ]
    with pytest.raises(ValueError, match="video_ids"):
        server.dispatch("thumbnails.cache", {"video_ids": "all"})


def test_thumbnail_cache_runs_during_workflow_and_reports_errors(
    monkeypatch, tmp_path
) -> None:
    repository = WatchlistRepository(tmp_path / "watchlist.json")
    repository.save(
        Watchlist(
            playlists=[
                WatchlistPlaylist(
                    playlist_id="PL123456789012345",
                    url="https://www.youtube.com/playlist?list=PL123456789012345",
                    items=[
                        WatchlistItem(
                            position=1,
                            title="Track",
                            video_id="dQw4w9WgXcQ",
                            thumbnail_url="https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg",
                        )
                    ],
                )
            ]
        )
    )
    workflow_started = Event()
    release_workflow = Event()

    def run(request, options, *, operations, events, cancellation):
        workflow_started.set()
        assert release_workflow.wait(5)

    async def cache(requests):
        return [
            ThumbnailResult(request.video_id, None, error="Image download failed")
            for request in requests
        ]

    monkeypatch.setattr(
        "muzik.native_gui.server.build_workflow_operations", lambda **kwargs: object()
    )
    monkeypatch.setattr("muzik.native_gui.server.run_workflow", run)
    monkeypatch.setattr("muzik.native_gui.server.cache_thumbnails", cache)
    monkeypatch.setattr("muzik.native_gui.server.cached_thumbnail_path", lambda _: None)
    writer = StringIO()
    server = NativeGuiServer(StringIO(), writer, repository=repository)
    try:
        job_id = server.dispatch("workflow.start", {"raw": "example"})["job_id"]
        assert workflow_started.wait(2)
        assert server.dispatch("thumbnails.cache", {"video_ids": ["dQw4w9WgXcQ"]}) == {
            "queued": 1
        }
        updated = _wait_for_record(
            server, writer, lambda record: record.get("event") == "thumbnails.updated"
        )
        assert updated["data"] == {
            "thumbnails": [
                {
                    "video_id": "dQw4w9WgXcQ",
                    "path": None,
                    "error": "Image download failed",
                }
            ]
        }
        assert server._job is not None and server._job.is_alive()
        assert server.dispatch("job.cancel", {"job_id": job_id})["cancel_requested"]
    finally:
        release_workflow.set()
        if server._job is not None:
            server._job.join(timeout=2)
        server._thumbnail_pool.shutdown(wait=True)


def test_library_scan_reports_file_details_and_total_size(tmp_path) -> None:
    audio = tmp_path / "Track [dQw4w9WgXcQ].mp3"
    audio.write_bytes(b"audio")
    server = NativeGuiServer(
        StringIO(),
        StringIO(),
        repository=WatchlistRepository(tmp_path / "watchlist.json"),
    )
    result = server.dispatch("library.scan", {"output": str(tmp_path)})
    assert result["output"] == str(tmp_path)
    assert result["total_size"] == "5.0 B"
    assert result["items"][0]["size_label"] == "5.0 B"
    assert result["items"][0]["youtube_id"] == "dQw4w9WgXcQ"
    assert result["items"][0]["modified"]

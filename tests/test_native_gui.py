"""Process protocol checks for the native GUI service."""

from io import StringIO
import json
from threading import Event, Thread
import time
from types import SimpleNamespace

import pytest

from muzik.core.watchlist import (
    StageStatus,
    Watchlist,
    WatchlistItem,
    WatchlistPlaylist,
    WatchlistRepository,
)
from muzik.core.sources.spotify_api import SpotifyPlaylistRef
from muzik.core.workflow.cancellation import CancellationToken
from muzik.core.workflow.events import MessageEvent
from muzik.native_gui.server import NativeGuiServer, _watchlist_data


def _records(writer: StringIO) -> list[dict]:
    return [json.loads(line) for line in writer.getvalue().splitlines()]


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
    records = _records(writer)
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
    status, playlists = _records(writer)
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

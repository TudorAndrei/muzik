"""Process protocol checks for the native GUI service."""

from io import StringIO
import json
from threading import Event, Thread
import time

import pytest

from muzik.core.watchlist import (
    Watchlist,
    WatchlistItem,
    WatchlistPlaylist,
    WatchlistRepository,
)
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


def test_watchlist_card_data_and_active_job_guard(monkeypatch, tmp_path) -> None:
    watchlist = Watchlist(
        playlists=[
            WatchlistPlaylist(
                playlist_id="PL123456789012345",
                url="https://www.youtube.com/playlist?list=PL123456789012345",
                items=[
                    WatchlistItem(position=1, title="Track", video_id="dQw4w9WgXcQ")
                ],
            )
        ]
    )
    card = _watchlist_data(watchlist)["playlists"][0]["items"][0]
    assert card["summary"] == "Pending"
    assert card["thumbnail_path"] is None
    assert "run" in card["actions"]

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

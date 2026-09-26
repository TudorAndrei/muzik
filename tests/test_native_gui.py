"""Process protocol checks for the native GUI service."""

from io import StringIO
import json
from threading import Thread
import time

from muzik.core.watchlist import WatchlistRepository
from muzik.core.workflow.cancellation import CancellationToken
from muzik.core.workflow.events import MessageEvent
from muzik.native_gui.server import NativeGuiServer


def _records(writer: StringIO) -> list[dict]:
    return [json.loads(line) for line in writer.getvalue().splitlines()]


def test_process_round_trip_and_watchlist_storage(tmp_path) -> None:
    reader = StringIO(
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
    assert [record["id"] for record in records] == ["a", "b", "c"]
    assert all(record["ok"] for record in records)
    assert records[0]["result"]["protocol_version"] == 1
    assert (
        records[2]["result"]["watchlist"]["playlists"][0]["playlist_id"]
        == "PL123456789012345"
    )


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

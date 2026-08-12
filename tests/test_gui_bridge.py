from queue import Queue
from threading import Event, Thread

import pytest

from muzik.core.workflow.cancellation import CancellationToken, WorkflowCancelled
from muzik.gui.bridge import GuiBridge


def test_submit_and_drain_keep_order() -> None:
    bridge = GuiBridge()
    calls: list[int] = []

    bridge.submit(lambda: calls.append(1))
    bridge.submit(lambda: calls.append(2))

    assert bridge.drain() == 2
    assert calls == [1, 2]


def test_request_returns_modal_result() -> None:
    bridge = GuiBridge()
    answer: Queue[str | BaseException] = Queue()

    def worker() -> None:
        try:
            answer.put(
                bridge.request(
                    lambda result: result.put("selected"),
                    CancellationToken(),
                )
            )
        except BaseException as exc:
            answer.put(exc)

    thread = Thread(target=worker)
    thread.start()
    bridge.drain()
    thread.join(timeout=1)

    assert not thread.is_alive()
    assert answer.get_nowait() == "selected"


def test_request_raises_when_cancelled() -> None:
    bridge = GuiBridge()
    cancellation = CancellationToken()
    opened = Event()
    answer: Queue[BaseException] = Queue()

    def worker() -> None:
        try:
            bridge.request(lambda result: opened.set(), cancellation)
        except BaseException as exc:
            answer.put(exc)

    thread = Thread(target=worker)
    thread.start()
    bridge.drain()
    assert opened.wait(1)
    cancellation.cancel()
    thread.join(timeout=1)

    assert not thread.is_alive()
    assert isinstance(answer.get_nowait(), WorkflowCancelled)


def test_late_submit_is_ignored_after_shutdown() -> None:
    bridge = GuiBridge()
    calls: list[str] = []

    bridge.shutdown()

    assert bridge.submit(lambda: calls.append("late")) is False
    assert bridge.drain() == 0
    assert calls == []


def test_shutdown_unblocks_request() -> None:
    bridge = GuiBridge()
    opened = Event()
    answer: Queue[BaseException] = Queue()

    def worker() -> None:
        try:
            bridge.request(lambda result: opened.set(), CancellationToken())
        except BaseException as exc:
            answer.put(exc)

    thread = Thread(target=worker)
    thread.start()
    bridge.drain()
    assert opened.wait(1)
    bridge.shutdown()
    thread.join(timeout=1)

    assert not thread.is_alive()
    assert isinstance(answer.get_nowait(), WorkflowCancelled)


def test_cancelled_token_rejects_request_before_modal() -> None:
    cancellation = CancellationToken()
    cancellation.cancel()

    with pytest.raises(WorkflowCancelled):
        GuiBridge().request(lambda result: None, cancellation)

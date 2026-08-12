"""Thread-safe bridge between workflow workers and the render thread."""

from __future__ import annotations

from collections.abc import Callable
import logging
from queue import Empty, Full, Queue
from threading import Lock
from typing import TypeVar, cast

from muzik.core.workflow.cancellation import CancellationToken, WorkflowCancelled


ResultT = TypeVar("ResultT")
_CANCELLED = object()
_LOGGER = logging.getLogger(__name__)


class GuiBridge:
    """Queue interface work and route blocking decisions to the render thread."""

    def __init__(
        self,
        *,
        on_error: Callable[[Exception], None] | None = None,
    ) -> None:
        self._work: Queue[Callable[[], None]] = Queue()
        self._pending: set[Queue[object]] = set()
        self._lock = Lock()
        self._shutdown = False
        self._on_error = on_error

    def is_shutdown(self) -> bool:
        with self._lock:
            return self._shutdown

    def submit(self, callback: Callable[[], None]) -> bool:
        """Queue a callback unless teardown has started."""
        with self._lock:
            if self._shutdown:
                return False
            self._work.put(callback)
            return True

    def drain(self) -> int:
        """Run all queued callbacks and return the number that were processed."""
        count = 0
        while True:
            try:
                callback = self._work.get_nowait()
            except Empty:
                return count
            if not self.is_shutdown():
                count += 1
                try:
                    callback()
                except WorkflowCancelled:
                    raise
                except Exception as exc:
                    self._report_error(exc)

    def request(
        self,
        show_modal: Callable[[Queue[ResultT]], None],
        cancellation: CancellationToken,
    ) -> ResultT:
        """Show a decision modal on the render thread and wait for its result."""
        cancellation.raise_if_cancelled()
        result: Queue[object] = Queue(maxsize=1)
        with self._lock:
            if self._shutdown:
                raise WorkflowCancelled("Workflow cancelled.")
            self._pending.add(result)

        def open_modal() -> None:
            if cancellation.is_cancelled() or self.is_shutdown():
                self._put_cancelled(result)
                return
            show_modal(cast(Queue[ResultT], result))

        if not self.submit(open_modal):
            self._forget(result)
            raise WorkflowCancelled("Workflow cancelled.")

        try:
            while True:
                cancellation.raise_if_cancelled()
                try:
                    value = result.get(timeout=0.05)
                except Empty:
                    continue
                if value is _CANCELLED:
                    raise WorkflowCancelled("Workflow cancelled.")
                return cast(ResultT, value)
        finally:
            self._forget(result)

    def cancel_pending(self) -> None:
        """Unblock all current decision requests."""
        with self._lock:
            pending = tuple(self._pending)
        for result in pending:
            self._put_cancelled(result)

    def shutdown(self) -> None:
        """Reject new work and unblock all current decision requests."""
        with self._lock:
            self._shutdown = True
        self.cancel_pending()
        while True:
            try:
                self._work.get_nowait()
            except Empty:
                break

    def _forget(self, result: Queue[object]) -> None:
        with self._lock:
            self._pending.discard(result)

    def _report_error(self, error: Exception) -> None:
        _LOGGER.exception(
            "DearPyGui render callback failed.",
            exc_info=(type(error), error, error.__traceback__),
        )
        if self._on_error is None:
            return
        try:
            self._on_error(error)
        except Exception:
            _LOGGER.exception("DearPyGui bridge error hook failed.")

    @staticmethod
    def _put_cancelled(result: Queue[object]) -> None:
        try:
            result.put_nowait(_CANCELLED)
        except Full:
            return

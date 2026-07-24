"""Cooperative cancellation primitives for long-running muzik workflows."""

from __future__ import annotations

from threading import Event


class WorkflowCancelled(RuntimeError):
    """Raised at a workflow safe boundary after cancellation is requested."""


class CancellationToken:
    """Thread-safe, idempotent cancellation signal shared by workflow adapters."""

    def __init__(self) -> None:
        self._event = Event()

    def cancel(self) -> None:
        self._event.set()

    def is_cancelled(self) -> bool:
        return self._event.is_set()

    def raise_if_cancelled(self) -> None:
        if self.is_cancelled():
            raise WorkflowCancelled("Workflow cancelled.")

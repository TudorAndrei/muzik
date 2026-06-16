"""Structured workflow events for CLI and GUI adapters."""

from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Literal, Protocol

from muzik.core.chapters import Chapter
from muzik.core.sources.base import Candidate


Severity = Literal["debug", "info", "warning", "error"]


@dataclass(frozen=True, slots=True)
class MessageEvent:
    message: str
    severity: Severity = "info"


@dataclass(frozen=True, slots=True)
class StepStartedEvent:
    name: str
    detail: str | None = None


@dataclass(frozen=True, slots=True)
class StepFinishedEvent:
    name: str
    detail: str | None = None
    success: bool = True


@dataclass(frozen=True, slots=True)
class ProgressStartedEvent:
    task_id: str
    description: str
    total: int | float | None = None


@dataclass(frozen=True, slots=True)
class ProgressAdvancedEvent:
    task_id: str
    advance: int | float = 1
    completed: int | float | None = None
    total: int | float | None = None


@dataclass(frozen=True, slots=True)
class ProgressFinishedEvent:
    task_id: str
    success: bool = True


@dataclass(frozen=True, slots=True)
class CandidatesFoundEvent:
    candidates: list[Candidate]
    source: str = "unknown"
    limit: int | None = None


@dataclass(frozen=True, slots=True)
class ChapterReviewRequestedEvent:
    source: Path
    chapters: list[Chapter]
    title: str | None = None


@dataclass(frozen=True, slots=True)
class ErrorEvent:
    message: str
    fatal: bool = False
    context: dict[str, Any] = field(default_factory=dict)


WorkflowEvent = (
    MessageEvent
    | StepStartedEvent
    | StepFinishedEvent
    | ProgressStartedEvent
    | ProgressAdvancedEvent
    | ProgressFinishedEvent
    | CandidatesFoundEvent
    | ChapterReviewRequestedEvent
    | ErrorEvent
)


class WorkflowEventEmitter(Protocol):
    def emit(self, event: WorkflowEvent) -> None: ...


class NullWorkflowEventEmitter:
    def emit(self, event: WorkflowEvent) -> None:
        return None


class RecordingWorkflowEventEmitter:
    def __init__(self) -> None:
        self.events: list[WorkflowEvent] = []

    def emit(self, event: WorkflowEvent) -> None:
        self.events.append(event)

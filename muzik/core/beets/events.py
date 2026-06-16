"""Structured events emitted by the beets integration."""

from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Literal, Protocol

from muzik.core.beets.views import BeetsDuplicateView, BeetsTaskView


Severity = Literal["debug", "info", "warning", "error"]


@dataclass(frozen=True, slots=True)
class BeetsLogEvent:
    message: str
    severity: Severity = "info"


@dataclass(frozen=True, slots=True)
class BeetsImportStartedEvent:
    paths: list[Path]
    dry_run: bool = False


@dataclass(frozen=True, slots=True)
class BeetsImportFinishedEvent:
    paths: list[Path]
    success: bool = True


@dataclass(frozen=True, slots=True)
class BeetsTaskEvent:
    task: BeetsTaskView


@dataclass(frozen=True, slots=True)
class BeetsDuplicateEvent:
    task: BeetsTaskView
    duplicates: list[BeetsDuplicateView] = field(default_factory=list)


@dataclass(frozen=True, slots=True)
class BeetsErrorEvent:
    message: str
    context: dict[str, Any] = field(default_factory=dict)


BeetsEvent = (
    BeetsLogEvent
    | BeetsImportStartedEvent
    | BeetsImportFinishedEvent
    | BeetsTaskEvent
    | BeetsDuplicateEvent
    | BeetsErrorEvent
)


class BeetsEventEmitter(Protocol):
    def emit(self, event: BeetsEvent) -> None: ...


class NullBeetsEventEmitter:
    def emit(self, event: BeetsEvent) -> None:
        return None


class RecordingBeetsEventEmitter:
    def __init__(self) -> None:
        self.events: list[BeetsEvent] = []

    def emit(self, event: BeetsEvent) -> None:
        self.events.append(event)

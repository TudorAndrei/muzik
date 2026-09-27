"""Import choices, views, and events shared by both import backends."""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum
from pathlib import Path
from typing import Any, Literal, Protocol


@dataclass(frozen=True, slots=True)
class MatchView:
    candidate_id: str
    artist: str | None = None
    album: str | None = None
    title: str | None = None
    distance: float | None = None


@dataclass(frozen=True, slots=True)
class TaskView:
    task_id: str
    paths: list[Path] = field(default_factory=list)
    is_album: bool = False
    item_count: int = 0
    current_artist: str | None = None
    current_album: str | None = None
    current_year: str | None = None
    matches: list[MatchView] = field(default_factory=list)


@dataclass(frozen=True, slots=True)
class DuplicateView:
    path: Path | None = None
    artist: str | None = None
    album: str | None = None
    title: str | None = None


class DuplicateDecision(str, Enum):
    SKIP = "skip"
    KEEP_ALL = "keep_all"
    REMOVE_OLD = "remove_old"
    MERGE = "merge"


class MatchDecision(str, Enum):
    AS_IS = "as_is"
    SKIP = "skip"


class ImportDecisions(Protocol):
    def should_resume_beets_import(self, path: Path) -> bool: ...

    def choose_beets_album_match(self, task: TaskView) -> Any: ...

    def choose_beets_track_match(self, task: TaskView) -> Any: ...

    def resolve_beets_duplicate(
        self, task: TaskView, duplicates: list[DuplicateView]
    ) -> DuplicateDecision: ...


Severity = Literal["debug", "info", "warning", "error"]


@dataclass(frozen=True, slots=True)
class LogEvent:
    message: str
    severity: Severity = "info"


@dataclass(frozen=True, slots=True)
class ImportStartedEvent:
    paths: list[Path]
    dry_run: bool = False


@dataclass(frozen=True, slots=True)
class ImportFinishedEvent:
    paths: list[Path]
    success: bool = True


@dataclass(frozen=True, slots=True)
class TaskEvent:
    task: TaskView


@dataclass(frozen=True, slots=True)
class DuplicateEvent:
    task: TaskView
    duplicates: list[DuplicateView] = field(default_factory=list)


@dataclass(frozen=True, slots=True)
class ErrorEvent:
    message: str
    context: dict[str, Any] = field(default_factory=dict)


ImportEvent = (
    LogEvent
    | ImportStartedEvent
    | ImportFinishedEvent
    | TaskEvent
    | DuplicateEvent
    | ErrorEvent
)


class ImportEventEmitter(Protocol):
    def emit(self, event: ImportEvent) -> None: ...


class NullImportEventEmitter:
    def emit(self, event: ImportEvent) -> None:
        return None


class RecordingImportEventEmitter:
    def __init__(self) -> None:
        self.events: list[ImportEvent] = []

    def emit(self, event: ImportEvent) -> None:
        self.events.append(event)


class NonInteractiveImportDecisions:
    """Keep tags as-is, or skip when quiet mode is active."""

    def __init__(
        self,
        *,
        quiet: bool = False,
        duplicate_decision: DuplicateDecision = DuplicateDecision.SKIP,
    ) -> None:
        self.quiet = quiet
        self.duplicate_decision = duplicate_decision

    def should_resume_beets_import(self, path: Path) -> bool:
        return False

    def choose_beets_album_match(self, task: TaskView) -> MatchDecision:
        return MatchDecision.SKIP if self.quiet else MatchDecision.AS_IS

    def choose_beets_track_match(self, task: TaskView) -> MatchDecision:
        return self.choose_beets_album_match(task)

    def resolve_beets_duplicate(
        self, task: TaskView, duplicates: list[DuplicateView]
    ) -> DuplicateDecision:
        return self.duplicate_decision


@dataclass(frozen=True, slots=True)
class ImportOptions:
    paths: list[Path]
    config_path: Path | None = None
    query: Any = None
    copy: bool = False
    link: bool = False
    move: bool = True
    nowrite: bool = False
    quiet: bool = False
    dry_run: bool = False
    incremental: bool = True
    autotag: bool = True
    duplicate_action: str | None = None

    def normalized(self) -> ImportOptions:
        return ImportOptions(
            paths=list(self.paths),
            config_path=self.config_path,
            query=self.query,
            copy=self.copy,
            link=self.link,
            move=self.move and not (self.copy or self.link),
            nowrite=self.nowrite,
            quiet=self.quiet,
            dry_run=self.dry_run,
            incremental=self.incremental,
            autotag=self.autotag,
            duplicate_action=self.duplicate_action,
        )

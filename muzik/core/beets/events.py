"""Compatibility names for structured import events."""

from muzik.core.import_models import (
    DuplicateEvent as BeetsDuplicateEvent,
    ErrorEvent as BeetsErrorEvent,
    ImportEvent as BeetsEvent,
    ImportEventEmitter as BeetsEventEmitter,
    ImportFinishedEvent as BeetsImportFinishedEvent,
    ImportStartedEvent as BeetsImportStartedEvent,
    LogEvent as BeetsLogEvent,
    NullImportEventEmitter as NullBeetsEventEmitter,
    RecordingImportEventEmitter as RecordingBeetsEventEmitter,
    Severity,
    TaskEvent as BeetsTaskEvent,
)

__all__ = [
    "BeetsDuplicateEvent",
    "BeetsErrorEvent",
    "BeetsEvent",
    "BeetsEventEmitter",
    "BeetsImportFinishedEvent",
    "BeetsImportStartedEvent",
    "BeetsLogEvent",
    "BeetsTaskEvent",
    "NullBeetsEventEmitter",
    "RecordingBeetsEventEmitter",
    "Severity",
]

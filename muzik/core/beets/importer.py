"""Beets importer integration."""

from __future__ import annotations

from dataclasses import dataclass
import os
from pathlib import Path
from threading import Lock
from typing import Any

from beets import config as beets_config
from beets import importer

from muzik.core.beets.config import open_library
from muzik.core.beets.decisions import (
    BeetsDecisions,
    BeetsDuplicateDecision,
    NonInteractiveBeetsDecisions,
)
from muzik.core.beets.events import (
    BeetsDuplicateEvent,
    BeetsEventEmitter,
    BeetsImportFinishedEvent,
    BeetsImportStartedEvent,
    BeetsTaskEvent,
    NullBeetsEventEmitter,
)
from muzik.core.beets.views import duplicate_view, task_view


_IMPORT_LOCK = Lock()


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

    def normalized(self) -> "ImportOptions":
        copy = self.copy
        link = self.link
        move = self.move
        if copy or link:
            move = False
        return ImportOptions(
            paths=list(self.paths),
            config_path=self.config_path,
            query=self.query,
            copy=copy,
            link=link,
            move=move,
            nowrite=self.nowrite,
            quiet=self.quiet,
            dry_run=self.dry_run,
            incremental=self.incremental,
        )


def apply_import_options(options: ImportOptions) -> None:
    """Apply CLI-compatible import flags to beets global import config."""
    options = options.normalized()
    import_config = beets_config["import"]
    import_config["copy"] = options.copy
    import_config["link"] = options.link
    import_config["move"] = options.move
    import_config["write"] = not options.nowrite
    import_config["quiet"] = options.quiet
    import_config["pretend"] = options.dry_run
    import_config["incremental"] = options.incremental


class MuzikImportSession(importer.ImportSession):
    def __init__(
        self,
        lib: Any,
        loghandler: Any,
        paths: list[Path],
        query: Any,
        decisions: BeetsDecisions,
        events: BeetsEventEmitter | None = None,
    ) -> None:
        super().__init__(lib, loghandler, [os.fsencode(path) for path in paths], query)
        self.decisions = decisions
        self.events = events or NullBeetsEventEmitter()

    def should_resume(self, path: bytes) -> bool:
        return self.decisions.should_resume_beets_import(Path(os.fsdecode(path)))

    def choose_match(self, task: Any) -> Any:
        self.events.emit(BeetsTaskEvent(task_view(task)))
        return self.decisions.choose_beets_album_match(task)

    def choose_item(self, task: Any) -> Any:
        self.events.emit(BeetsTaskEvent(task_view(task)))
        return self.decisions.choose_beets_track_match(task)

    def resolve_duplicate(self, task: Any, found_duplicates: list[Any]) -> None:
        duplicates = [duplicate_view(duplicate) for duplicate in found_duplicates]
        self.events.emit(BeetsDuplicateEvent(task_view(task), duplicates))
        decision = self.decisions.resolve_beets_duplicate(task, found_duplicates)
        apply_duplicate_decision(task, decision)


def apply_duplicate_decision(task: Any, decision: BeetsDuplicateDecision) -> None:
    if decision == BeetsDuplicateDecision.SKIP:
        task.set_choice(importer.Action.SKIP)
    elif decision == BeetsDuplicateDecision.KEEP_ALL:
        return
    elif decision == BeetsDuplicateDecision.REMOVE_OLD:
        task.should_remove_duplicates = True
    elif decision == BeetsDuplicateDecision.MERGE:
        task.should_merge_duplicates = True
    else:
        raise ValueError(f"unknown duplicate decision: {decision}")


def import_paths(
    options: ImportOptions,
    *,
    decisions: BeetsDecisions | None = None,
    events: BeetsEventEmitter | None = None,
) -> None:
    options = options.normalized()
    decisions = decisions or NonInteractiveBeetsDecisions(quiet=options.quiet)
    events = events or NullBeetsEventEmitter()
    with _IMPORT_LOCK:
        apply_import_options(options)
        lib = open_library(options.config_path)
        session = MuzikImportSession(
            lib,
            None,
            options.paths,
            options.query,
            decisions,
            events,
        )
        events.emit(BeetsImportStartedEvent(options.paths, dry_run=options.dry_run))
        try:
            session.run()
        finally:
            events.emit(BeetsImportFinishedEvent(options.paths))

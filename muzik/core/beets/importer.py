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
    BeetsMatchDecision,
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
from muzik.core.beets.views import BeetsTaskView, duplicate_view, task_view


_IMPORT_LOCK = Lock()


class BeetsImporterAdapter:
    """Keep live Beets objects worker-local behind stable opaque view IDs."""

    def __init__(self) -> None:
        self._next_task_id = 0
        self._task_ids: dict[int, str] = {}
        self._candidates: dict[str, dict[str, Any]] = {}

    def view_for(self, task: Any) -> BeetsTaskView:
        key = id(task)
        task_id = self._task_ids.get(key)
        if task_id is None:
            task_id = f"task-{self._next_task_id}"
            self._next_task_id += 1
            self._task_ids[key] = task_id
        view = task_view(task, task_id=task_id)
        self._candidates[task_id] = {
            match.candidate_id: candidate
            for match, candidate in zip(
                view.matches,
                getattr(task, "candidates", []) or [],
                strict=True,
            )
        }
        return view

    def resolve_choice(self, task: Any, choice: Any) -> Any:
        if choice is None:
            return importer.Action.SKIP
        if choice == BeetsMatchDecision.AS_IS:
            return importer.Action.ASIS
        if not isinstance(choice, str):
            return choice
        task_id = self._task_ids.get(id(task))
        if task_id is None:
            raise ValueError("Unknown Beets task ID.")
        try:
            return self._candidates[task_id][choice]
        except KeyError as exc:
            raise ValueError(f"Unknown Beets candidate ID: {choice}") from exc


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
        self.adapter = BeetsImporterAdapter()

    def should_resume(self, path: bytes) -> bool:
        return self.decisions.should_resume_beets_import(Path(os.fsdecode(path)))

    def choose_match(self, task: Any) -> Any:
        view = self.adapter.view_for(task)
        self.events.emit(BeetsTaskEvent(view))
        return self.adapter.resolve_choice(
            task, self.decisions.choose_beets_album_match(view)
        )

    def choose_item(self, task: Any) -> Any:
        view = self.adapter.view_for(task)
        self.events.emit(BeetsTaskEvent(view))
        return self.adapter.resolve_choice(
            task, self.decisions.choose_beets_track_match(view)
        )

    def resolve_duplicate(self, task: Any, found_duplicates: list[Any]) -> None:
        view = self.adapter.view_for(task)
        duplicates = [duplicate_view(duplicate) for duplicate in found_duplicates]
        self.events.emit(BeetsDuplicateEvent(view, duplicates))
        decision = self.decisions.resolve_beets_duplicate(view, duplicates)
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
        lib = open_library(options.config_path)
        apply_import_options(options)
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
        except Exception:
            events.emit(BeetsImportFinishedEvent(options.paths, success=False))
            raise
        else:
            events.emit(BeetsImportFinishedEvent(options.paths))

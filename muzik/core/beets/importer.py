"""Beets importer integration."""

from __future__ import annotations

from dataclasses import dataclass
import os
from pathlib import Path
from threading import Lock
from typing import Any

from beets import config as beets_config
from beets import importer
from requests.exceptions import RequestException

from muzik.config import get_native_settings
from muzik.core.matching import NativeRanking, rank_album_candidates
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
    BeetsLogEvent,
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

    def view_for(
        self, task: Any, ranking: NativeRanking | None = None
    ) -> BeetsTaskView:
        key = id(task)
        task_id = self._task_ids.get(key)
        if task_id is None:
            task_id = f"task-{self._next_task_id}"
            self._next_task_id += 1
            self._task_ids[key] = task_id
        ranked = (
            [(row.original_index, row.distance) for row in ranking.candidates]
            if ranking is not None
            else None
        )
        view = task_view(task, task_id=task_id, ranked=ranked)
        self._candidates[task_id] = {
            f"{task_id}:match:{index}": candidate
            for index, candidate in enumerate(getattr(task, "candidates", []) or [])
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
    autotag: bool = True
    # When set, override the config's duplicate_action (e.g. "remove" so a
    # re-download replaces the existing library album instead of being skipped).
    duplicate_action: str | None = None

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
            autotag=self.autotag,
            duplicate_action=self.duplicate_action,
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
    import_config["autotag"] = options.autotag
    if options.duplicate_action is not None:
        import_config["duplicate_action"] = options.duplicate_action


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
        self.match_mode = get_native_settings()["match"]

    def should_resume(self, path: bytes) -> bool:
        return self.decisions.should_resume_beets_import(Path(os.fsdecode(path)))

    def choose_match(self, task: Any) -> Any:
        ranking = rank_album_candidates(task) if self.match_mode == "native" else None
        view = self.adapter.view_for(task, ranking)
        if self.match_mode == "shadow":
            try:
                ranking = rank_album_candidates(task)
                if _ranking_differs(view, ranking):
                    self.events.emit(
                        BeetsLogEvent(
                            "Native album ranking differs from beets.",
                            severity="warning",
                        )
                    )
            except Exception as exc:  # noqa: BLE001 - shadow mode must keep beets usable
                self.events.emit(
                    BeetsLogEvent(
                        f"Native album ranking failed: {exc}",
                        severity="warning",
                    )
                )
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


def _ranking_differs(view: BeetsTaskView, ranking: NativeRanking) -> bool:
    if len(view.matches) != len(ranking.candidates):
        return True
    for index, candidate in enumerate(ranking.candidates):
        if candidate.original_index != index:
            return True
        if view.matches[index].distance != candidate.distance:
            return True
    return False


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


class PruneAborted(Exception):
    """Raised when too many files look missing to prune safely."""

    def __init__(self, missing: int, total: int) -> None:
        super().__init__(f"{missing}/{total} items missing")
        self.missing = missing
        self.total = total


def _item_fullpath(item: Any, directory: str) -> str:
    path = os.fsdecode(item.path)
    return path if os.path.isabs(path) else os.path.join(directory, path)


def prune_missing_items(
    config_path: Path | None = None,
    *,
    safety_fraction: float = 0.5,
) -> int:
    """Remove library items whose files no longer exist; return the count.

    A move-mode re-tag can leave the old entry orphaned once beets moves the
    file to its new path. Pruning those keeps the library consistent. As a
    safeguard against a whole unmounted volume, abort with ``PruneAborted`` when
    more than ``safety_fraction`` of items look missing.
    """
    with _IMPORT_LOCK:
        lib = open_library(config_path)
        directory = os.fsdecode(lib.directory)
        items = list(lib.items())
        missing = [
            item
            for item in items
            if not os.path.exists(_item_fullpath(item, directory))
        ]
        if items and len(missing) > len(items) * safety_fraction:
            raise PruneAborted(len(missing), len(items))
        for item in missing:
            item.remove(delete=False, with_album=True)
    return len(missing)


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

        def make_session() -> MuzikImportSession:
            return MuzikImportSession(
                lib,
                None,
                options.paths,
                options.query,
                decisions,
                events,
            )

        session = make_session()
        events.emit(BeetsImportStartedEvent(options.paths, dry_run=options.dry_run))
        try:
            try:
                session.run()
            except RequestException:
                events.emit(
                    BeetsLogEvent(
                        "MusicBrainz is unavailable. Importing with the current tags.",
                        severity="warning",
                    )
                )
                beets_config["import"]["autotag"] = False
                try:
                    make_session().run()
                finally:
                    beets_config["import"]["autotag"] = options.autotag
        except Exception:
            events.emit(BeetsImportFinishedEvent(options.paths, success=False))
            raise
        else:
            events.emit(BeetsImportFinishedEvent(options.paths))

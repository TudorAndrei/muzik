"""Pass native import plans to the existing import decision interface."""

from __future__ import annotations

import importlib
import json
import os
from pathlib import Path
import pickle
from typing import TYPE_CHECKING, Any, Callable

from muzik.config import LIBRARY_CONFIG
from muzik.core.import_models import (
    DuplicateDecision,
    DuplicateEvent,
    DuplicateView,
    ErrorEvent,
    ImportFinishedEvent,
    ImportOptions,
    ImportStartedEvent,
    LogEvent,
    MatchDecision,
    MatchView,
    TaskEvent,
    TaskView,
)

if TYPE_CHECKING:
    from muzik.core.import_models import ImportDecisions, ImportEventEmitter


def _new_importer(config_path: Path, overrides: dict[str, Any]) -> Any:
    native = importlib.import_module("muzik._native")
    importer = native.NativeImporter(str(config_path), json.dumps(overrides))
    if overrides.get("import", {}).get("incremental"):
        statefile = Path(importer.statefile_path())
        history = Path(importer.history_path())
        if statefile.is_file() and not history.exists():
            importer.seed_incremental_history(_read_legacy_history(statefile))
    return importer


def _read_legacy_history(statefile: Path) -> list[list[str]]:
    """Read the old import history once before native history exists."""
    with statefile.open("rb") as stream:
        state = pickle.load(stream)
    if not isinstance(state, dict):
        raise ValueError(f"invalid import state at {statefile}")
    entries = state.get("taghistory", set())
    if not isinstance(entries, set):
        raise ValueError(f"invalid import history at {statefile}")
    groups: list[list[str]] = []
    for entry in entries:
        if not isinstance(entry, tuple) or not entry:
            raise ValueError(f"invalid import history entry at {statefile}")
        if not all(isinstance(path, (bytes, str)) for path in entry):
            raise ValueError(f"invalid import history path at {statefile}")
        groups.append([os.fsdecode(path) for path in entry])
    return groups


def _overrides(options: ImportOptions) -> dict[str, Any]:
    return {
        "import": {
            "copy": options.copy,
            "link": options.link,
            "move": options.move,
            "write": not options.nowrite,
            "pretend": options.dry_run,
            "incremental": options.incremental,
            "autotag": options.autotag,
        }
    }


def _plan_paths(importer: Any, paths: list[Path]) -> list[dict[str, Any]]:
    files = [path.is_file() for path in paths]
    if any(files):
        if not all(files):
            raise ValueError("cannot mix files and directories in one import")
        return importer.plan_singletons([str(path) for path in paths])
    return importer.plan([str(path) for path in paths])


def _task_view(index: int, album: dict[str, Any]) -> TaskView:
    task_id = f"native:{index}"
    matches = [
        MatchView(
            candidate_id=f"{task_id}:match:{candidate_index}",
            artist=candidate.get("artist"),
            album=candidate.get("album"),
            distance=candidate.get("distance"),
        )
        for candidate_index, candidate in enumerate(album["candidates"])
    ]
    return TaskView(
        task_id=task_id,
        paths=[Path(path) for path in album["paths"]],
        is_album=True,
        item_count=len(album["paths"]),
        current_artist=album.get("current_artist"),
        current_album=album.get("current_album"),
        current_year=album.get("current_year"),
        matches=matches,
    )


def _choice(value: Any, task: TaskView) -> tuple[str, int | None]:
    if value is None or value == MatchDecision.SKIP:
        return "skip", None
    if value == MatchDecision.AS_IS:
        return "as_is", None
    for index, match in enumerate(task.matches):
        if value == match.candidate_id:
            return "candidate", index
    raise ValueError("unknown native import candidate")


def _duplicate_choice(value: DuplicateDecision) -> str:
    choices = {
        DuplicateDecision.SKIP: "skip",
        DuplicateDecision.KEEP_ALL: "keep",
        DuplicateDecision.REMOVE_OLD: "replace",
    }
    try:
        return choices[value]
    except KeyError as exc:
        raise ValueError(f"native import cannot use duplicate action {value}") from exc


def preview_sync(config_path: Path, query: str) -> tuple[int, int]:
    """Count library records selected for sync without changing them."""
    from muzik.core.native_library import NativeLibrary

    library = NativeLibrary(config_path)
    return len(library.albums(query)), len(library.items(query))


def run_native_import(
    options: ImportOptions,
    decisions: ImportDecisions,
    events: ImportEventEmitter,
    *,
    factory: Callable[[Path, dict[str, Any]], Any] = _new_importer,
) -> None:
    """Plan, choose, and apply an import with the native bridge."""
    importer = factory(options.config_path or LIBRARY_CONFIG, _overrides(options))
    events.emit(ImportStartedEvent(options.paths, dry_run=options.dry_run))
    try:
        if options.query is not None:
            if options.dry_run:
                albums, items = preview_sync(
                    options.config_path or LIBRARY_CONFIG, str(options.query)
                )
                events.emit(
                    LogEvent(
                        f"Sync preview: {albums} albums and {items} items selected."
                    )
                )
            else:
                importer.sync(str(options.query), not options.nowrite)
        else:
            albums = _plan_paths(importer, options.paths)
            choices: list[tuple[str, int | None, str | None]] = []
            for index, album in enumerate(albums):
                task = _task_view(index, album)
                events.emit(TaskEvent(task))
                choice, candidate_index = _choice(
                    decisions.choose_album_match(task), task
                )
                duplicate_choice = None
                if choice != "skip" and album["duplicates"]:
                    duplicates = [
                        DuplicateView(
                            path=Path(row["path"]) if row.get("path") else None,
                            artist=row.get("artist"),
                            album=row.get("album"),
                        )
                        for row in album["duplicates"]
                    ]
                    events.emit(DuplicateEvent(task, duplicates))
                    if options.duplicate_action is not None:
                        aliases = {
                            "remove": "replace",
                            "skip": "skip",
                            "keep": "keep",
                        }
                        duplicate_choice = aliases.get(options.duplicate_action)
                        if duplicate_choice is None:
                            raise ValueError(
                                "native import cannot use duplicate action "
                                f"{options.duplicate_action}"
                            )
                    else:
                        duplicate_choice = _duplicate_choice(
                            decisions.resolve_duplicate(task, duplicates)
                        )
                choices.append((choice, candidate_index, duplicate_choice))
            result = importer.apply(choices)
            if options.dry_run:
                destinations = result.get("destinations", [])
                events.emit(
                    LogEvent(
                        f"Import preview: {len(albums)} groups and "
                        f"{len(destinations)} destinations."
                    )
                )
                for destination in destinations:
                    events.emit(LogEvent(f"  {destination}"))
            cleanup_failed = result.get("cleanup_failed", [])
            if cleanup_failed:
                events.emit(
                    LogEvent(
                        f"Native import could not clean up {len(cleanup_failed)} old files.",
                        severity="warning",
                    )
                )
            source_cleanup_failed = result.get("source_cleanup_failed", [])
            if source_cleanup_failed:
                events.emit(
                    LogEvent(
                        "Native import could not remove "
                        f"{len(source_cleanup_failed)} moved source files.",
                        severity="warning",
                    )
                )
    except Exception as exc:
        events.emit(ErrorEvent(str(exc)))
        events.emit(ImportFinishedEvent(options.paths, success=False))
        raise
    events.emit(ImportFinishedEvent(options.paths))

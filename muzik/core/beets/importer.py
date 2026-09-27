"""Beets importer integration."""

from __future__ import annotations

from dataclasses import replace
import os
from pathlib import Path
import sqlite3
import tempfile
from threading import Lock
from typing import Any
from urllib.parse import quote

from beets import config as beets_config
from beets import importer
from beets import plugins as beets_plugins
from beets.library import Library as BeetsLibrary
from requests.exceptions import RequestException

from muzik.config import get_native_settings
from muzik.core.import_models import ImportOptions
from muzik.core.matching import NativeRanking, rank_album_candidates
from muzik.core.beets.config import load_config, open_library
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
        if choice is None or choice == BeetsMatchDecision.SKIP:
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
        shadow: NativeShadowComparison | None = None,
    ) -> None:
        super().__init__(lib, loghandler, [os.fsencode(path) for path in paths], query)
        self.decisions = decisions
        self.events = events or NullBeetsEventEmitter()
        self.adapter = BeetsImporterAdapter()
        self.match_mode = get_native_settings()["match"]
        self.shadow = shadow

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
        choice = self.adapter.resolve_choice(
            task, self.decisions.choose_beets_album_match(view)
        )
        if self.shadow is not None:
            self.shadow.observe(task, choice)
        return choice

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


class NativeShadowComparison:
    """Compare native plans with beets results in a temporary database."""

    def __init__(self, native: Any, albums: list[dict[str, Any]]) -> None:
        self.native = native
        self.albums = albums
        self.groups = {
            frozenset(Path(path).resolve() for path in album["paths"]): index
            for index, album in enumerate(albums)
        }
        self.observed: dict[int, Any] = {}
        self.compared = 0
        self.release_differences = 0
        self.destination_differences = 0
        self.unmatched = 0

    def observe(self, task: Any, choice: Any) -> None:
        paths = frozenset(Path(os.fsdecode(path)).resolve() for path in task.paths)
        index = self.groups.get(paths)
        if index is None:
            self.unmatched += 1
            return
        self.observed[index] = choice
        album = self.albums[index]
        self.compared += 1
        native_candidates = album["candidates"]
        beets_candidates = list(getattr(task, "candidates", []) or [])
        native_top = native_candidates[0]["id"] if native_candidates else None
        beets_top = (
            getattr(getattr(beets_candidates[0], "info", None), "album_id", None)
            if beets_candidates
            else None
        )
        selected_id = getattr(getattr(choice, "info", None), "album_id", None)
        if native_top != beets_top or (
            selected_id is not None and selected_id != native_top
        ):
            self.release_differences += 1

    def compare_destinations(self, library: Any, original_ids: set[int]) -> None:
        imported = [item for item in library.items() if item.id not in original_ids]
        choices: list[tuple[str, int | None, str | None]] = []
        actual_by_group: dict[int, list[Path]] = {}
        for index, album in enumerate(self.albums):
            sources = {Path(path).resolve() for path in album["paths"]}
            rows = [
                item
                for item in imported
                if Path(os.fsdecode(item.path)).resolve() in sources
            ]
            if not rows:
                choices.append(("skip", None, None))
                continue
            if index not in self.observed:
                self.observed[index] = importer.Action.ASIS
                self.compared += 1
            actual_by_group[index] = [
                Path(os.fsdecode(item.destination())) for item in rows
            ]
            chosen = self.observed.get(index)
            if chosen is None or chosen == importer.Action.ASIS:
                choice = ("as_is", None, "keep" if album["duplicates"] else None)
            else:
                release_id = getattr(getattr(chosen, "info", None), "album_id", None)
                candidate_index = next(
                    (
                        candidate_index
                        for candidate_index, candidate in enumerate(album["candidates"])
                        if candidate["id"] == release_id
                    ),
                    None,
                )
                if candidate_index is None:
                    self.release_differences += 1
                    choice = ("skip", None, None)
                else:
                    choice = (
                        "candidate",
                        candidate_index,
                        "keep" if album["duplicates"] else None,
                    )
            choices.append(choice)
        result = self.native.apply(choices)
        offset = 0
        for index, (album, choice) in enumerate(zip(self.albums, choices)):
            if choice[0] == "skip":
                continue
            count = len(album["paths"])
            expected = [
                Path(path) for path in result["destinations"][offset : offset + count]
            ]
            offset += count
            if sorted(expected) != sorted(actual_by_group.get(index, [])):
                self.destination_differences += 1

    def report(self, events: BeetsEventEmitter) -> None:
        unmatched = self.unmatched + len(self.albums) - len(self.observed)
        severity = (
            "warning"
            if unmatched or self.release_differences or self.destination_differences
            else "info"
        )
        events.emit(
            BeetsLogEvent(
                "Native import shadow: "
                f"{self.compared} groups compared, "
                f"{unmatched} unmatched, "
                f"{self.release_differences} release differences, "
                f"{self.destination_differences} destination differences.",
                severity=severity,
            )
        )


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
        if get_native_settings()["library"] == "native":
            from muzik.core.native_library import NativeLibrary

            removed, aborted = NativeLibrary(config_path).prune_missing_items(
                safety_fraction
            )
            if aborted is not None:
                raise PruneAborted(*aborted)
            return removed
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


def _run_shadow_import(
    options: ImportOptions,
    decisions: BeetsDecisions,
    events: BeetsEventEmitter,
) -> None:
    """Run beets against a database snapshot with file writes disabled."""
    from muzik.core.native_import import preview_native_plan

    load_config(options.config_path)
    beets_plugins.load_plugins()
    source = Path(beets_config["library"].as_filename())
    directory = beets_config["directory"].as_filename()
    shadow = None
    if options.query is None:
        try:
            native, albums = preview_native_plan(options)
            shadow = NativeShadowComparison(native, albums)
        except Exception as exc:
            events.emit(
                BeetsLogEvent(
                    f"Native import shadow plan failed: {type(exc).__name__}",
                    severity="warning",
                )
            )
    else:
        events.emit(
            BeetsLogEvent(
                "Native import shadow comparison is unavailable for query sync.",
                severity="warning",
            )
        )
    events.emit(BeetsImportStartedEvent(options.paths, dry_run=True))
    with tempfile.TemporaryDirectory(
        prefix="muzik-import-shadow-", dir="/private/tmp"
    ) as name:
        snapshot = Path(name) / "library.db"
        uri = "file:" + quote(str(source), safe="/") + "?mode=ro"
        with sqlite3.connect(uri, uri=True) as original:
            with sqlite3.connect(snapshot) as target:
                original.backup(target)
        library = BeetsLibrary(str(snapshot), directory)
        original_ids = {item.id for item in library.items() if item.id is not None}
        safe_options = replace(
            options,
            copy=False,
            link=False,
            move=False,
            nowrite=True,
            dry_run=False,
            incremental=False,
        )
        old_import = beets_config["import"].get()
        old_state = beets_config["statefile"].get()
        old_stages = beets_plugins.import_stages
        old_early_stages = beets_plugins.early_import_stages
        old_send = beets_plugins.send
        try:
            apply_import_options(safe_options)
            beets_config["statefile"] = str(Path(name) / "state.pickle")
            beets_config["import"]["hardlink"] = False
            beets_config["import"]["reflink"] = False
            setattr(beets_plugins, "import_stages", lambda: [])
            setattr(beets_plugins, "early_import_stages", lambda: [])

            def safe_send(event: str, **arguments: Any) -> list[Any]:
                if event == "import_task_created":
                    return old_send(event, **arguments)
                return []

            setattr(beets_plugins, "send", safe_send)
            session = MuzikImportSession(
                library, None, options.paths, options.query, decisions, events, shadow
            )
            session.run()
            if shadow is not None:
                try:
                    shadow.compare_destinations(library, original_ids)
                except Exception as exc:
                    events.emit(
                        BeetsLogEvent(
                            "Native import shadow destination comparison failed: "
                            f"{type(exc).__name__}",
                            severity="warning",
                        )
                    )
                shadow.report(events)
        except Exception:
            events.emit(BeetsImportFinishedEvent(options.paths, success=False))
            raise
        else:
            events.emit(BeetsImportFinishedEvent(options.paths))
        finally:
            setattr(beets_plugins, "import_stages", old_stages)
            setattr(beets_plugins, "early_import_stages", old_early_stages)
            setattr(beets_plugins, "send", old_send)
            beets_config["import"] = old_import
            beets_config["statefile"] = old_state
            library._close()


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
        import_mode = get_native_settings()["import"]
        if import_mode == "native":
            from muzik.core.native_import import run_native_import

            run_native_import(options, decisions, events)
            return
        if import_mode == "shadow":
            _run_shadow_import(options, decisions, events)
            return
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

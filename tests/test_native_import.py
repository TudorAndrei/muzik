"""Decision and event flow for the native import bridge."""

from pathlib import Path
import json
import pickle
import shutil
from typing import Any

import pytest

from muzik.core.import_models import (
    DuplicateDecision,
    DuplicateEvent,
    ImportFinishedEvent,
    ImportOptions,
    LogEvent,
    MatchDecision,
    RecordingImportEventEmitter,
    TaskEvent,
)
from muzik.core import native_import
from muzik.core.native_import import run_native_import


def test_first_native_import_creates_library(tmp_path: Path) -> None:
    from muzik import _native

    source = tmp_path / "incoming" / "track.flac"
    source.parent.mkdir()
    fixture = (
        Path(__file__).resolve().parents[1]
        / "rust/crates/muzik-tags/tests/fixtures/mediafile.flac"
    )
    shutil.copyfile(fixture, source)
    database = tmp_path / "data" / "library.db"
    config = tmp_path / "config.yaml"
    overrides = {
        "library": str(database),
        "directory": str(tmp_path / "music"),
        "statefile": str(tmp_path / "state.pickle"),
        "import": {"autotag": False, "copy": True, "write": False},
    }
    config.write_text(json.dumps(overrides), encoding="utf-8")
    importer = _native.NativeImporter(str(config), json.dumps(overrides))
    plan = importer.plan([str(source)])
    assert len(plan) == 1
    assert not database.exists()

    result = importer.apply([("as_is", None, None)])
    assert database.exists()
    assert len(result["item_ids"]) == 1
    assert Path(result["destinations"][0]).exists()
    assert len(_native.NativeLibrary(str(config)).items("")) == 1


class FakeImporter:
    def __init__(self) -> None:
        self.paths: list[str] | None = None
        self.plan_mode: str | None = None
        self.choices: list[tuple[str, int | None, str | None]] | None = None

    def plan(self, paths: list[str]) -> list[dict[str, Any]]:
        self.paths = paths
        self.plan_mode = "albums"
        return self._albums()

    def plan_singletons(self, paths: list[str]) -> list[dict[str, Any]]:
        self.paths = paths
        self.plan_mode = "singletons"
        return self._albums()

    def _albums(self) -> list[dict[str, Any]]:
        return [
            {
                "paths": ["/music/new/01.flac"],
                "current_artist": "Current",
                "current_album": "Album",
                "current_year": "2020",
                "candidates": [
                    {"artist": "Artist", "album": "Release", "distance": 0.05}
                ],
                "duplicates": [
                    {"path": "/music/old/01.flac", "artist": "Old", "album": "Album"}
                ],
            }
        ]

    def apply(
        self, choices: list[tuple[str, int | None, str | None]]
    ) -> dict[str, Any]:
        self.choices = choices
        return {"cleanup_failed": []}

    def sync(self, query: str, write: bool) -> None:
        pytest.fail("sync wrote in dry-run mode")


class Decisions:
    def should_resume_beets_import(self, path: Path) -> bool:
        return False

    def choose_beets_album_match(self, task: Any) -> str:
        return task.matches[0].candidate_id

    def choose_beets_track_match(self, task: Any) -> str:
        return self.choose_beets_album_match(task)

    def resolve_beets_duplicate(self, task: Any, duplicates: Any) -> DuplicateDecision:
        return DuplicateDecision.REMOVE_OLD


def test_native_import_maps_candidate_and_duplicate_decisions() -> None:
    native = FakeImporter()
    events = RecordingImportEventEmitter()
    options = ImportOptions(paths=[Path("/music/new")], copy=True).normalized()
    run_native_import(
        options, Decisions(), events, factory=lambda path, overrides: native
    )
    assert native.paths == ["/music/new"]
    assert native.plan_mode == "albums"
    assert native.choices == [("candidate", 0, "replace")]
    assert any(isinstance(event, TaskEvent) for event in events.events)
    assert any(isinstance(event, DuplicateEvent) for event in events.events)
    assert isinstance(events.events[-1], ImportFinishedEvent)
    assert events.events[-1].success


def test_native_import_plans_file_as_singleton(tmp_path: Path) -> None:
    track = tmp_path / "track.flac"
    track.write_bytes(b"audio")
    native = FakeImporter()
    run_native_import(
        ImportOptions(paths=[track]),
        Decisions(),
        RecordingImportEventEmitter(),
        factory=lambda path, overrides: native,
    )
    assert native.plan_mode == "singletons"
    assert native.paths == [str(track)]


def test_native_import_previews_dry_run_query_without_writing(monkeypatch) -> None:
    native = FakeImporter()
    events = RecordingImportEventEmitter()
    options = ImportOptions(paths=[], query="album:Test", dry_run=True)
    monkeypatch.setattr(native_import, "preview_sync", lambda path, query: (2, 5))
    run_native_import(
        options, Decisions(), events, factory=lambda path, overrides: native
    )
    assert isinstance(events.events[-1], ImportFinishedEvent)
    assert events.events[-1].success
    assert any(
        isinstance(event, LogEvent)
        and event.message == "Sync preview: 2 albums and 5 items selected."
        for event in events.events
    )


def test_native_import_reports_moved_sources_left_after_commit() -> None:
    class CleanupImporter(FakeImporter):
        def apply(
            self, choices: list[tuple[str, int | None, str | None]]
        ) -> dict[str, Any]:
            super().apply(choices)
            return {"source_cleanup_failed": ["/private/source.flac"]}

    events = RecordingImportEventEmitter()
    run_native_import(
        ImportOptions(paths=[Path("/music/new")]),
        Decisions(),
        events,
        factory=lambda path, overrides: CleanupImporter(),
    )
    warnings = [
        event.message
        for event in events.events
        if isinstance(event, LogEvent) and event.severity == "warning"
    ]
    assert warnings == ["Native import could not remove 1 moved source files."]


def test_native_as_is_choice() -> None:
    class AsIsDecisions(Decisions):
        def choose_beets_album_match(self, task: Any) -> Any:
            return MatchDecision.AS_IS

        def resolve_beets_duplicate(self, task: Any, duplicates: Any) -> Any:
            return DuplicateDecision.KEEP_ALL

    native = FakeImporter()
    run_native_import(
        ImportOptions(paths=[Path("/music/new")]),
        AsIsDecisions(),
        RecordingImportEventEmitter(),
        factory=lambda path, overrides: native,
    )
    assert native.choices == [("as_is", None, "keep")]


def test_legacy_incremental_history_seeds_native_import(
    tmp_path: Path, monkeypatch
) -> None:
    statefile = tmp_path / "state.pickle"
    album = tmp_path / "old-album"
    track = album / "01.flac"
    album.mkdir()
    track.write_bytes(b"audio")
    with statefile.open("wb") as stream:
        pickle.dump(
            {
                "tagprogress": {},
                "taghistory": {(str(album).encode(), str(track).encode())},
            },
            stream,
        )

    seeded: list[list[str]] = []

    class Native:
        def __init__(self, config_path: str, overrides: str) -> None:
            pass

        def statefile_path(self) -> str:
            return str(statefile)

        def history_path(self) -> str:
            return str(tmp_path / "state.muzik-history.json")

        def seed_incremental_history(self, groups: list[list[str]]) -> None:
            seeded.extend(groups)

    monkeypatch.setattr(
        native_import.importlib,
        "import_module",
        lambda name: type("Bridge", (), {"NativeImporter": Native}),
    )
    native_import._new_importer(
        tmp_path / "config.yaml", {"import": {"incremental": True}}
    )
    assert seeded == [[str(album), str(track)]]

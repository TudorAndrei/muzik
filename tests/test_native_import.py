"""Decision and event flow for the native import bridge."""

from pathlib import Path
import shutil
import sqlite3
from types import SimpleNamespace
from typing import Any

import pytest
from beets.importer import Action
from mediafile import MediaFile

from muzik.core.beets.decisions import (
    BeetsDuplicateDecision,
    BeetsMatchDecision,
)
from muzik.core.beets.events import (
    BeetsLogEvent,
    BeetsDuplicateEvent,
    BeetsImportFinishedEvent,
    BeetsTaskEvent,
    RecordingBeetsEventEmitter,
)
from muzik.core.beets import importer as beets_importer
from muzik.core.beets.importer import NativeShadowComparison
from muzik.core.import_models import ImportOptions, RecordingImportEventEmitter
from muzik.core.native_import import run_native_import


class FakeImporter:
    def __init__(self) -> None:
        self.paths: list[str] | None = None
        self.choices: list[tuple[str, int | None, str | None]] | None = None

    def plan(self, paths: list[str]) -> list[dict[str, Any]]:
        self.paths = paths
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


class Decisions:
    def should_resume_beets_import(self, path: Path) -> bool:
        return False

    def choose_beets_album_match(self, task: Any) -> str:
        return task.matches[0].candidate_id

    def choose_beets_track_match(self, task: Any) -> str:
        return self.choose_beets_album_match(task)

    def resolve_beets_duplicate(self, task: Any, duplicates: Any) -> Any:
        return BeetsDuplicateDecision.REMOVE_OLD


def test_native_import_maps_candidate_and_duplicate_decisions() -> None:
    native = FakeImporter()
    events = RecordingImportEventEmitter()
    options = ImportOptions(paths=[Path("/music/new")], copy=True).normalized()

    run_native_import(
        options,
        Decisions(),
        events,
        factory=lambda path, overrides: native,
    )

    assert native.paths == ["/music/new"]
    assert native.choices == [("candidate", 0, "replace")]
    assert any(isinstance(event, BeetsTaskEvent) for event in events.events)
    assert any(isinstance(event, BeetsDuplicateEvent) for event in events.events)
    assert isinstance(events.events[-1], BeetsImportFinishedEvent)
    assert events.events[-1].success


def test_native_import_keeps_dry_run_query_from_writing() -> None:
    native = FakeImporter()
    events = RecordingImportEventEmitter()
    options = ImportOptions(paths=[], query="album:Test", dry_run=True)

    with pytest.raises(ValueError, match="dry run"):
        run_native_import(
            options,
            Decisions(),
            events,
            factory=lambda path, overrides: native,
        )

    assert isinstance(events.events[-1], BeetsImportFinishedEvent)
    assert not events.events[-1].success


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
        if isinstance(event, BeetsLogEvent) and event.severity == "warning"
    ]
    assert warnings == ["Native import could not remove 1 moved source files."]


def test_native_as_is_choice_uses_existing_decision_type() -> None:
    class AsIsDecisions(Decisions):
        def choose_beets_album_match(self, task: Any) -> Any:
            return BeetsMatchDecision.AS_IS

        def resolve_beets_duplicate(self, task: Any, duplicates: Any) -> Any:
            return BeetsDuplicateDecision.KEEP_ALL

    native = FakeImporter()
    run_native_import(
        ImportOptions(paths=[Path("/music/new")]),
        AsIsDecisions(),
        RecordingImportEventEmitter(),
        factory=lambda path, overrides: native,
    )
    assert native.choices == [("as_is", None, "keep")]


def test_import_entry_selects_native_mode(monkeypatch: pytest.MonkeyPatch) -> None:
    options = beets_importer.ImportOptions(paths=[Path("/music/new")])
    expected = Decisions()
    calls: list[beets_importer.ImportOptions] = []

    monkeypatch.setattr(
        beets_importer,
        "get_native_settings",
        lambda: {"import": "native"},
    )
    monkeypatch.setattr(
        "muzik.core.native_import.run_native_import",
        lambda selected, decisions, events: calls.append(selected),
    )
    monkeypatch.setattr(
        beets_importer,
        "open_library",
        lambda path: (_ for _ in ()).throw(AssertionError("beets opened")),
    )
    beets_importer.import_paths(options, decisions=expected)
    assert calls == [options.normalized()]


def test_shadow_compares_release_and_destination_counts_without_paths() -> None:
    planned = [
        {
            "paths": ["/music/source/01.flac"],
            "candidates": [{"id": "release-1"}],
            "duplicates": [],
        }
    ]
    native = SimpleNamespace(
        apply=lambda choices: {"destinations": ["/music/library/01.flac"]}
    )
    shadow = NativeShadowComparison(native, planned)
    task = SimpleNamespace(
        paths=[b"/music/source/01.flac"],
        candidates=[SimpleNamespace(info=SimpleNamespace(album_id="release-1"))],
    )
    shadow.observe(task, Action.ASIS)
    library = SimpleNamespace(
        items=lambda: [
            SimpleNamespace(
                id=1,
                path=b"/music/source/01.flac",
                destination=lambda: b"/music/library/01.flac",
            )
        ]
    )
    shadow.compare_destinations(library, set())
    events = RecordingBeetsEventEmitter()
    shadow.report(events)
    assert shadow.compared == 1
    assert shadow.release_differences == 0
    assert shadow.destination_differences == 0
    report = events.events[0]
    assert isinstance(report, BeetsLogEvent)
    assert "/music/" not in report.message


def test_shadow_counts_a_selected_release_that_differs_from_native_top() -> None:
    planned = [
        {
            "paths": ["/music/source/01.flac"],
            "candidates": [{"id": "release-1"}, {"id": "release-2"}],
            "duplicates": [],
        }
    ]
    applied: list[tuple[str, int | None, str | None]] = []

    def apply(choices: list[tuple[str, int | None, str | None]]) -> dict[str, Any]:
        applied.extend(choices)
        return {"destinations": ["/music/library/01.flac"]}

    shadow = NativeShadowComparison(SimpleNamespace(apply=apply), planned)
    shadow.observe(
        SimpleNamespace(
            paths=[b"/music/source/01.flac"],
            candidates=[SimpleNamespace(info=SimpleNamespace(album_id="release-1"))],
        ),
        SimpleNamespace(info=SimpleNamespace(album_id="release-2")),
    )
    library = SimpleNamespace(
        items=lambda: [
            SimpleNamespace(
                id=1,
                path=b"/music/source/01.flac",
                destination=lambda: b"/music/library/01.flac",
            )
        ]
    )
    shadow.compare_destinations(library, set())

    assert shadow.release_differences == 1
    assert shadow.destination_differences == 0
    assert applied == [("candidate", 1, None)]


def test_shadow_runs_beets_in_dry_run_mode(monkeypatch: pytest.MonkeyPatch) -> None:
    calls: list[beets_importer.ImportOptions] = []

    monkeypatch.setattr(
        beets_importer, "get_native_settings", lambda: {"import": "shadow"}
    )
    monkeypatch.setattr(
        beets_importer,
        "_run_shadow_import",
        lambda options, decisions, events: calls.append(options),
    )
    events = RecordingBeetsEventEmitter()

    beets_importer.import_paths(
        beets_importer.ImportOptions(paths=[Path("/music/new")]),
        decisions=Decisions(),
        events=events,
    )

    assert calls == [beets_importer.ImportOptions(paths=[Path("/music/new")])]


def test_shadow_plan_error_keeps_source_and_database_unchanged(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    fixture = (
        Path(__file__).resolve().parents[1]
        / "rust/crates/muzik-library/tests/fixtures/library.db"
    )
    database = tmp_path / "library.db"
    shutil.copyfile(fixture, database)
    source = tmp_path / "new_album"
    source.mkdir()
    track = source / "track.flac"
    tags_fixture = (
        Path(__file__).resolve().parents[1]
        / "rust/crates/muzik-tags/tests/fixtures/mediafile.flac"
    )
    shutil.copyfile(tags_fixture, track)
    tags = MediaFile(str(track))
    tags.artist = "Parity Artist"
    tags.albumartist = "Parity Artist"
    tags.album = "Parity Album"
    tags.title = "Parity Track"
    tags.mb_albumid = None
    tags.mb_trackid = None
    tags.save()
    music = tmp_path / "music"
    music.mkdir()
    config = tmp_path / "beets.yaml"
    config.write_text(f"library: {database}\ndirectory: {music}\n", encoding="utf-8")
    with sqlite3.connect(database) as connection:
        original_count = connection.execute("SELECT count(*) FROM items").fetchone()[0]
    monkeypatch.setenv("MUZIK_NATIVE_IMPORT", "shadow")

    def fail(_options: Any) -> Any:
        raise RuntimeError("native planner unavailable")

    monkeypatch.setattr("muzik.core.native_import.preview_native_plan", fail)
    events = RecordingBeetsEventEmitter()
    beets_importer.import_paths(
        beets_importer.ImportOptions(
            paths=[source], config_path=config, nowrite=True, autotag=False
        ),
        events=events,
    )

    with sqlite3.connect(database) as connection:
        assert (
            connection.execute("SELECT count(*) FROM items").fetchone()[0]
            == original_count
        )
    assert track.exists()
    assert list(music.iterdir()) == []
    assert any(
        isinstance(event, BeetsLogEvent) and "shadow plan failed" in event.message
        for event in events.events
    )
    assert isinstance(events.events[-1], BeetsImportFinishedEvent)
    assert events.events[-1].success

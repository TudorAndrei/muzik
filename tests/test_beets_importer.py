from pathlib import Path

from beets import config as beets_config
from beets import importer as beets_importer
import pytest

from muzik.core.beets.decisions import BeetsDuplicateDecision, BeetsMatchDecision
from muzik.core.beets.events import (
    BeetsDuplicateEvent,
    BeetsImportFinishedEvent,
    BeetsImportStartedEvent,
    BeetsTaskEvent,
    RecordingBeetsEventEmitter,
)
from muzik.core.beets.importer import (
    BeetsImporterAdapter,
    ImportOptions,
    MuzikImportSession,
    apply_duplicate_decision,
    apply_import_options,
    import_paths,
)


class FakeDecisions:
    def __init__(self) -> None:
        self.calls: list[str] = []
        self.duplicate_decision = BeetsDuplicateDecision.SKIP

    def should_resume_beets_import(self, path: Path) -> bool:
        self.calls.append(f"resume:{path}")
        return True

    def choose_beets_album_match(self, task):
        self.calls.append("album")
        return beets_importer.Action.APPLY

    def choose_beets_track_match(self, task):
        self.calls.append("track")
        return beets_importer.Action.ASIS

    def resolve_beets_duplicate(self, task, duplicates):
        self.calls.append(f"duplicate:{len(duplicates)}")
        return self.duplicate_decision


class FakeTask:
    paths = ["/tmp/Album"]
    is_album = True
    candidates = []

    def __init__(self) -> None:
        self.choice = None
        self.should_remove_duplicates = False
        self.should_merge_duplicates = False

    def set_choice(self, choice):
        self.choice = choice


def test_import_options_normalizes_move_copy_link() -> None:
    options = ImportOptions(paths=[Path("Album")], copy=True, move=True)

    normalized = options.normalized()

    assert normalized.copy is True
    assert normalized.link is False
    assert normalized.move is False


def test_apply_import_options_maps_flags_to_beets_config() -> None:
    apply_import_options(
        ImportOptions(
            paths=[Path("Album")],
            copy=False,
            link=True,
            move=True,
            nowrite=True,
            quiet=True,
            dry_run=True,
            incremental=False,
        )
    )

    import_config = beets_config["import"]
    assert import_config["copy"].get(bool) is False
    assert import_config["link"].get(bool) is True
    assert import_config["move"].get(bool) is False
    assert import_config["write"].get(bool) is False
    assert import_config["quiet"].get(bool) is True
    assert import_config["pretend"].get(bool) is True
    assert import_config["incremental"].get(bool) is False


def test_apply_import_options_sets_duplicate_action_when_given() -> None:
    beets_config["import"]["duplicate_action"] = "skip"
    apply_import_options(ImportOptions(paths=[Path("Album")]))
    assert beets_config["import"]["duplicate_action"].get(str) == "skip"

    apply_import_options(
        ImportOptions(paths=[Path("Album")], duplicate_action="remove")
    )
    assert beets_config["import"]["duplicate_action"].get(str) == "remove"


def test_muzik_import_session_delegates_decisions_and_emits_events() -> None:
    decisions = FakeDecisions()
    events = RecordingBeetsEventEmitter()
    session = MuzikImportSession(
        object(),
        None,
        [Path("/tmp/Album")],
        None,
        decisions,
        events,
    )
    task = FakeTask()

    assert session.should_resume(b"/tmp/Album") is True
    assert session.choose_match(task) is beets_importer.Action.APPLY
    assert session.choose_item(task) is beets_importer.Action.ASIS
    session.resolve_duplicate(task, [object()])

    assert decisions.calls == [
        "resume:/tmp/Album",
        "album",
        "track",
        "duplicate:1",
    ]
    assert task.choice is beets_importer.Action.SKIP
    assert [type(event) for event in events.events] == [
        BeetsTaskEvent,
        BeetsTaskEvent,
        BeetsDuplicateEvent,
    ]


def test_apply_duplicate_decision_sets_task_flags() -> None:
    task = FakeTask()

    apply_duplicate_decision(task, BeetsDuplicateDecision.SKIP)
    assert task.choice is beets_importer.Action.SKIP

    task = FakeTask()

    apply_duplicate_decision(task, BeetsDuplicateDecision.REMOVE_OLD)
    assert task.should_remove_duplicates is True

    task = FakeTask()
    apply_duplicate_decision(task, BeetsDuplicateDecision.MERGE)
    assert task.should_merge_duplicates is True

    task = FakeTask()
    apply_duplicate_decision(task, BeetsDuplicateDecision.KEEP_ALL)
    assert task.choice is None


def test_importer_adapter_exposes_only_opaque_view_ids() -> None:
    task = FakeTask()
    candidate = object()
    task.candidates = [candidate]
    adapter = BeetsImporterAdapter()

    view = adapter.view_for(task)

    assert view.task_id == "task-0"
    assert view.matches[0].candidate_id == "task-0:match:0"
    assert not hasattr(view, "raw")
    assert adapter.resolve_choice(task, view.matches[0].candidate_id) is candidate
    assert (
        adapter.resolve_choice(task, BeetsMatchDecision.AS_IS)
        is beets_importer.Action.ASIS
    )
    with pytest.raises(ValueError, match="Unknown Beets candidate ID"):
        adapter.resolve_choice(task, "stale-id")


def test_import_paths_applies_options_runs_session_and_emits_events(
    monkeypatch,
) -> None:
    calls: list[str] = []

    class FakeSession:
        def __init__(self, lib, loghandler, paths, query, decisions, events):
            calls.append(f"session:{paths}:{query}")

        def run(self):
            calls.append("run")

    monkeypatch.setattr("muzik.core.beets.importer.open_library", lambda path: "lib")
    monkeypatch.setattr("muzik.core.beets.importer.MuzikImportSession", FakeSession)
    events = RecordingBeetsEventEmitter()

    import_paths(
        ImportOptions(
            paths=[Path("Album")],
            config_path=Path("config.yaml"),
            dry_run=True,
        ),
        decisions=FakeDecisions(),
        events=events,
    )

    assert calls == ["session:[PosixPath('Album')]:None", "run"]
    assert [type(event) for event in events.events] == [
        BeetsImportStartedEvent,
        BeetsImportFinishedEvent,
    ]
    assert isinstance(events.events[-1], BeetsImportFinishedEvent)
    assert events.events[-1].success is True


def test_import_paths_applies_runtime_options_after_loading_config(monkeypatch) -> None:
    calls: list[str] = []

    class FakeSession:
        def __init__(self, *args):
            return None

        def run(self):
            return None

    def fake_open_library(path):
        calls.append("load-config")
        return "lib"

    def fake_apply(options):
        calls.append(
            "runtime-options:"
            f"{options.copy}:{options.link}:{options.move}:{options.nowrite}:"
            f"{options.quiet}:{options.dry_run}:{options.incremental}"
        )

    monkeypatch.setattr("muzik.core.beets.importer.open_library", fake_open_library)
    monkeypatch.setattr("muzik.core.beets.importer.apply_import_options", fake_apply)
    monkeypatch.setattr("muzik.core.beets.importer.MuzikImportSession", FakeSession)

    import_paths(
        ImportOptions(
            paths=[Path("Album")],
            copy=True,
            link=False,
            move=True,
            nowrite=True,
            quiet=True,
            dry_run=True,
            incremental=False,
        )
    )

    assert calls == [
        "load-config",
        "runtime-options:True:False:False:True:True:True:False",
    ]


def test_import_paths_emits_failed_event_and_reraises(monkeypatch) -> None:
    class FailingSession:
        def __init__(self, *args):
            return None

        def run(self):
            raise RuntimeError("beets failed")

    monkeypatch.setattr("muzik.core.beets.importer.open_library", lambda path: "lib")
    monkeypatch.setattr("muzik.core.beets.importer.MuzikImportSession", FailingSession)
    events = RecordingBeetsEventEmitter()

    with pytest.raises(RuntimeError, match="beets failed"):
        import_paths(ImportOptions(paths=[Path("Album")]), events=events)

    assert events.events == [
        BeetsImportStartedEvent([Path("Album")], dry_run=False),
        BeetsImportFinishedEvent([Path("Album")], success=False),
    ]

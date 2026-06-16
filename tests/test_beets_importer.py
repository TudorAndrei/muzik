from pathlib import Path

from beets import config as beets_config
from beets import importer as beets_importer

from muzik.core.beets.decisions import BeetsDuplicateDecision
from muzik.core.beets.events import (
    BeetsDuplicateEvent,
    BeetsImportFinishedEvent,
    BeetsImportStartedEvent,
    BeetsTaskEvent,
    RecordingBeetsEventEmitter,
)
from muzik.core.beets.importer import (
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

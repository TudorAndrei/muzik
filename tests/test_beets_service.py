from pathlib import Path

import pytest

from muzik.core.beets.importer import ImportOptions
from muzik.core.beets.decisions import NonInteractiveBeetsDecisions
from muzik.core.beets.events import RecordingBeetsEventEmitter
from muzik.core.beets.service import OrganizationError, organize_paths


def test_organize_paths_delegates_import_with_decisions_and_events(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    album = tmp_path / "Album"
    album.mkdir()
    calls = []
    decisions = NonInteractiveBeetsDecisions()
    events = RecordingBeetsEventEmitter()

    monkeypatch.setattr(
        "muzik.core.beets.service.import_paths",
        lambda options, *, decisions, events: calls.append(
            (options, decisions, events)
        ),
    )

    organize_paths(
        ImportOptions(paths=[album]),
        decisions=decisions,
        events=events,
    )

    assert calls == [(ImportOptions(paths=[album]), decisions, events)]


def test_organize_paths_keeps_tag_only_runner_injected(tmp_path: Path) -> None:
    album = tmp_path / "Album"
    album.mkdir()
    calls = []

    organize_paths(
        ImportOptions(paths=[album]),
        tag_only=True,
        tag_only_runner=lambda path, options: calls.append((path, options)),
    )

    assert calls == [(album, ImportOptions(paths=[album]))]


def test_organize_paths_rejects_missing_directory(tmp_path: Path) -> None:
    with pytest.raises(OrganizationError, match="Directory not found"):
        organize_paths(ImportOptions(paths=[tmp_path / "missing"]))

from pathlib import Path

import pytest

from muzik.core.workflow import operations
from muzik.core.workflow.decisions import NonInteractiveWorkflowDecisions
from muzik.core.workflow.service import WorkflowOptions, WorkflowServiceError


def test_gui_workflow_keeps_beets_error_detail(tmp_path: Path, monkeypatch) -> None:
    album = tmp_path / "splits" / "High-Flying"
    album.mkdir(parents=True)

    def fail_organization(options, **kwargs) -> None:
        raise RuntimeError("library database is locked")

    monkeypatch.setattr(operations, "organize_paths", fail_organization)
    workflow_operations = operations.build_workflow_operations(
        splits=tmp_path / "splits",
        options=WorkflowOptions(),
        decisions=NonInteractiveWorkflowDecisions(),
    )

    with pytest.raises(
        WorkflowServiceError,
        match="Beets could not organize High-Flying: library database is locked",
    ):
        workflow_operations.process_audio([], [album])


def test_gui_workflow_repairs_placeholder_tags_before_beets(
    tmp_path: Path, monkeypatch
) -> None:
    album = tmp_path / "splits" / "Kohsuke Mine - Sunshower (1976)"
    album.mkdir(parents=True)
    calls: list[str] = []

    def repair(target: Path):
        calls.append(f"repair:{target.name}")
        return type(
            "Repair",
            (),
            {
                "updated_files": 4,
                "artist": "Kohsuke Mine",
                "album": "Sunshower",
                "year": "1976",
            },
        )()

    def organize(options, **kwargs) -> None:
        calls.append(f"organize:{options.paths[0].name}")

    monkeypatch.setattr(operations, "repair_placeholder_album_tags", repair)
    monkeypatch.setattr(operations, "organize_paths", organize)
    workflow_operations = operations.build_workflow_operations(
        splits=tmp_path / "splits",
        options=WorkflowOptions(),
        decisions=NonInteractiveWorkflowDecisions(),
    )

    workflow_operations.process_audio([], [album])

    assert calls == [
        "repair:Kohsuke Mine - Sunshower (1976)",
        "organize:Kohsuke Mine - Sunshower (1976)",
    ]

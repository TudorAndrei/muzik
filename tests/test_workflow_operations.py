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

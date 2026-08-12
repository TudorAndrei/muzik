from pathlib import Path

import pytest

from muzik.core.beets.decisions import BeetsDuplicateDecision, BeetsMatchDecision
from muzik.core.beets.views import BeetsDuplicateView, BeetsMatchView, BeetsTaskView
from muzik.core.chapters import Chapter
from muzik.core.sources.base import Candidate
from muzik.core.workflow.cancellation import CancellationToken
from muzik.core.workflow.decisions import ChapterDecision, WorkflowDecisionError
from muzik.gui.adapters import GuiBeetsDecisions, GuiWorkflowDecisions


class FakeBridge:
    def __init__(self, value=None) -> None:
        self.value = value
        self.requests = 0

    def request(self, show_modal, cancellation):
        self.requests += 1
        return self.value


def test_workflow_non_interactive_defaults() -> None:
    bridge = FakeBridge()
    decisions = GuiWorkflowDecisions(bridge, interactive=False)
    candidate = Candidate(source="soulseek", source_id="1", title="Album")
    chapters = [Chapter(index=1, start=0, end=None, title="Track")]

    assert decisions.choose_soulseek_candidate([candidate]) is candidate
    assert (
        decisions.confirm_chapters(Path("album.flac"), chapters)
        is ChapterDecision.ACCEPT
    )
    assert decisions.edit_chapters(chapters) is chapters
    assert bridge.requests == 0


def test_workflow_candidate_requires_a_value() -> None:
    decisions = GuiWorkflowDecisions(FakeBridge(None), interactive=True)
    candidate = Candidate(source="soulseek", source_id="1", title="Album")

    with pytest.raises(WorkflowDecisionError):
        decisions.choose_soulseek_candidate([candidate])


def test_workflow_results_route_through_bridge() -> None:
    candidate = Candidate(source="soulseek", source_id="2", title="Second")
    chapter_bridge = FakeBridge(ChapterDecision.EDIT)
    candidate_bridge = FakeBridge(candidate)

    assert (
        GuiWorkflowDecisions(candidate_bridge).choose_soulseek_candidate([candidate])
        is candidate
    )
    assert (
        GuiWorkflowDecisions(chapter_bridge).confirm_chapters(Path("a.flac"), [])
        is ChapterDecision.EDIT
    )
    assert candidate_bridge.requests == 1
    assert chapter_bridge.requests == 1


def test_beets_non_interactive_defaults() -> None:
    bridge = FakeBridge()
    decisions = GuiBeetsDecisions(bridge, interactive=False)
    task = BeetsTaskView(task_id="task")

    assert decisions.should_resume_beets_import(Path("album")) is False
    assert decisions.choose_beets_album_match(task) is BeetsMatchDecision.AS_IS
    assert decisions.choose_beets_track_match(task) is BeetsMatchDecision.AS_IS
    assert (
        decisions.resolve_beets_duplicate(
            task,
            [BeetsDuplicateView(title="Track")],
        )
        is BeetsDuplicateDecision.SKIP
    )
    assert bridge.requests == 0


def test_beets_results_route_through_bridge() -> None:
    task = BeetsTaskView(
        task_id="task",
        matches=[BeetsMatchView(candidate_id="candidate-1")],
    )
    match_bridge = FakeBridge("candidate-1")
    duplicate_bridge = FakeBridge(BeetsDuplicateDecision.MERGE)

    assert (
        GuiBeetsDecisions(match_bridge).choose_beets_album_match(task) == "candidate-1"
    )
    assert (
        GuiBeetsDecisions(duplicate_bridge).resolve_beets_duplicate(task, [])
        is BeetsDuplicateDecision.MERGE
    )
    assert match_bridge.requests == 1
    assert duplicate_bridge.requests == 1


def test_cancelled_decision_does_not_request_modal() -> None:
    cancellation = CancellationToken()
    cancellation.cancel()
    bridge = FakeBridge()

    with pytest.raises(RuntimeError, match="cancelled"):
        GuiBeetsDecisions(
            bridge,
            cancellation=cancellation,
        ).choose_beets_album_match(BeetsTaskView(task_id="task"))
    assert bridge.requests == 0

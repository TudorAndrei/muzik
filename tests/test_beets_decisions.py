"""Non-interactive beets decision behaviour."""

from beets import importer

from muzik.core.beets.decisions import NonInteractiveBeetsDecisions
from muzik.core.beets.views import BeetsMatchView, BeetsTaskView


def _task(*matches: BeetsMatchView) -> BeetsTaskView:
    return BeetsTaskView(task_id="t1", is_album=True, matches=list(matches))


def test_applies_best_candidate_by_id() -> None:
    task = _task(
        BeetsMatchView(candidate_id="t1:match:0", distance=0.1),
        BeetsMatchView(candidate_id="t1:match:1", distance=0.4),
    )
    # Must return a candidate id, never Action.APPLY (internal-only in beets).
    assert NonInteractiveBeetsDecisions().choose_beets_album_match(task) == "t1:match:0"


def test_imports_as_is_without_a_candidate() -> None:
    decisions = NonInteractiveBeetsDecisions()
    assert decisions.choose_beets_album_match(_task()) is importer.Action.ASIS
    assert decisions.choose_beets_track_match(_task()) is importer.Action.ASIS


def test_quiet_skips() -> None:
    task = _task(BeetsMatchView(candidate_id="t1:match:0", distance=0.1))
    decisions = NonInteractiveBeetsDecisions(quiet=True)
    assert decisions.choose_beets_album_match(task) is importer.Action.SKIP

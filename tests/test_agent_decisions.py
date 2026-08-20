"""Tests for the LLM-backed beets import decisions (no network)."""

from pathlib import Path

from muzik.core.beets.agent_decisions import (
    AgentBeetsDecisions,
    MatchDecision,
    build_prompt,
)
from muzik.core.beets.decisions import BeetsMatchDecision
from muzik.core.beets.views import BeetsMatchView, BeetsTaskView


def _task(*matches: BeetsMatchView) -> BeetsTaskView:
    return BeetsTaskView(
        task_id="t1",
        paths=[Path("/music/Some Artist - Album/01 One.flac")],
        is_album=True,
        matches=list(matches),
    )


def _match(index: int, distance: float | None) -> BeetsMatchView:
    return BeetsMatchView(
        candidate_id=f"t1:match:{index}",
        artist="Some Artist",
        album="Album",
        title=None,
        distance=distance,
    )


def test_no_candidates_skips() -> None:
    called = []
    dec = AgentBeetsDecisions(chooser=lambda task: called.append(task) or None)
    assert dec.choose_beets_album_match(_task()) is None
    assert called == []  # never consulted the agent


def test_strong_match_applies_without_agent() -> None:
    called = []
    dec = AgentBeetsDecisions(chooser=lambda task: called.append(task) or None)
    task = _task(_match(0, 0.05), _match(1, 0.4))
    assert dec.choose_beets_album_match(task) == "t1:match:0"
    assert called == []  # strong match never calls the agent


def test_agent_pick_with_high_confidence_applies() -> None:
    decision = MatchDecision(action="pick", candidate_index=1, confidence=0.9)
    dec = AgentBeetsDecisions(chooser=lambda task: decision)
    task = _task(_match(0, 0.5), _match(1, 0.35))
    assert dec.choose_beets_album_match(task) == "t1:match:1"


def test_agent_pick_with_low_confidence_skips() -> None:
    decision = MatchDecision(action="pick", candidate_index=1, confidence=0.3)
    dec = AgentBeetsDecisions(chooser=lambda task: decision)
    task = _task(_match(0, 0.5), _match(1, 0.35))
    assert dec.choose_beets_album_match(task) is None


def test_agent_invalid_index_skips() -> None:
    decision = MatchDecision(action="pick", candidate_index=9, confidence=0.99)
    dec = AgentBeetsDecisions(chooser=lambda task: decision)
    task = _task(_match(0, 0.5))
    assert dec.choose_beets_album_match(task) is None


def test_agent_as_is_returns_as_is() -> None:
    decision = MatchDecision(action="as_is", confidence=0.8)
    dec = AgentBeetsDecisions(chooser=lambda task: decision)
    task = _task(_match(0, 0.5))
    assert dec.choose_beets_album_match(task) is BeetsMatchDecision.AS_IS


def test_agent_skip_returns_none() -> None:
    decision = MatchDecision(action="skip", confidence=0.2)
    dec = AgentBeetsDecisions(chooser=lambda task: decision)
    task = _task(_match(0, 0.5))
    assert dec.choose_beets_album_match(task) is None


def test_agent_error_falls_back_to_skip() -> None:
    def boom(task: BeetsTaskView) -> MatchDecision:
        raise RuntimeError("network down")

    dec = AgentBeetsDecisions(chooser=boom)
    task = _task(_match(0, 0.5))
    assert dec.choose_beets_album_match(task) is None


def test_build_prompt_lists_candidates() -> None:
    task = _task(_match(0, 0.5), _match(1, 0.35))
    prompt = build_prompt(task)
    assert "Some Artist - Album" in prompt
    assert "[0]" in prompt and "[1]" in prompt
    assert "01 One.flac" in prompt

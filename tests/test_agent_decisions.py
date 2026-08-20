"""Tests for the LLM-backed beets import decisions (no network)."""

from pathlib import Path

from muzik.core.beets.agent_decisions import (
    AgentBeetsDecisions,
    MatchDecision,
    _cli_chooser,
    build_prompt,
    cli_prompt,
    decision_from_text,
    extract_action_json,
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


def test_extract_action_json_ignores_other_objects_and_ansi() -> None:
    text = (
        "\x1b[32msome log line\x1b[0m\n"
        '{"unrelated": true}\n'
        'assistant: here is the answer {"action": "skip", "confidence": 0.1, '
        '"reason": "no match"}\n'
    )
    obj = extract_action_json(text)
    assert obj is not None
    assert obj["action"] == "skip"


def test_extract_action_json_returns_last_action_object() -> None:
    text = '{"action": "skip"} ... later {"action": "pick", "candidate_index": 2}'
    obj = extract_action_json(text)
    assert obj is not None
    assert obj["action"] == "pick" and obj["candidate_index"] == 2


def test_decision_from_text_validates() -> None:
    text = 'noise {"action": "pick", "candidate_index": 0, "confidence": 0.9} tail'
    decision = decision_from_text(text, lambda _m: None)
    assert isinstance(decision, MatchDecision)
    assert decision.action == "pick" and decision.candidate_index == 0


def test_decision_from_text_none_when_absent() -> None:
    assert decision_from_text("no json here", lambda _m: None) is None


def test_cli_chooser_parses_runner_output_via_stdin() -> None:
    seen: dict[str, object] = {}

    def runner(cmd: list[str], stdin_text: str | None) -> str:
        seen["cmd"] = cmd
        seen["stdin"] = stdin_text
        return '{"action": "as_is", "confidence": 0.7, "reason": "already tagged"}'

    chooser = _cli_chooser(
        lambda: ["codex", "exec"],
        pass_prompt_via_stdin=True,
        log=lambda _m: None,
        runner=runner,
    )
    decision = chooser(_task(_match(0, 0.4)))
    assert isinstance(decision, MatchDecision)
    assert decision.action == "as_is"
    assert seen["cmd"] == ["codex", "exec"]  # prompt went through stdin, not argv
    assert isinstance(seen["stdin"], str) and "Candidates:" in seen["stdin"]


def test_cli_chooser_passes_prompt_as_arg_when_not_stdin() -> None:
    seen: dict[str, object] = {}

    def runner(cmd: list[str], stdin_text: str | None) -> str:
        seen["cmd"] = cmd
        seen["stdin"] = stdin_text
        return '{"action": "skip", "confidence": 0.2, "reason": "unsure"}'

    chooser = _cli_chooser(
        lambda: ["opencode", "run"],
        pass_prompt_via_stdin=False,
        log=lambda _m: None,
        runner=runner,
    )
    assert chooser(_task(_match(0, 0.4))) is not None
    assert seen["stdin"] is None
    cmd = seen["cmd"]
    assert isinstance(cmd, list) and cmd[:2] == ["opencode", "run"]
    last = cmd[-1]
    assert isinstance(last, str) and "Candidates:" in last  # prompt is last arg


def test_cli_chooser_returns_none_when_cli_missing() -> None:
    chooser = _cli_chooser(
        lambda: ["nope"],
        pass_prompt_via_stdin=True,
        log=lambda _m: None,
        runner=lambda cmd, stdin: None,  # simulates CLI not found / empty output
    )
    assert chooser(_task(_match(0, 0.4))) is None


def test_cli_prompt_demands_json() -> None:
    prompt = cli_prompt(_task(_match(0, 0.4)))
    assert "JSON object" in prompt
    assert "Candidates:" in prompt

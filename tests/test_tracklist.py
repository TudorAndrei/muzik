"""Tests for description tracklist parsing (regex and agent stages)."""

from muzik.core import tracklist
from muzik.core.tracklist import (
    TrackList,
    _regex_tracklist,
    _tracklist_to_chapters,
    chapters_from_description,
)


REAL_DESCRIPTION = """Tracklist:
0:00 - Twilight
3:20 - Horizon
6:34 - Locals
9:43 - Sublime
12:42 - Mind Drifting (w/B.Young)
30:37 - Vintage

This album is a collection of my favorite boom bap beats from 2019."""


def test_regex_parses_timestamp_dash_title() -> None:
    chapters = _regex_tracklist(REAL_DESCRIPTION)
    assert chapters is not None
    assert [c.title for c in chapters[:3]] == ["Twilight", "Horizon", "Locals"]
    assert chapters[0].start == 0 and chapters[0].end == 200  # 3:20
    assert chapters[4].title == "Mind Drifting (w/B.Young)"
    assert chapters[-1].title == "Vintage" and chapters[-1].end is None


def test_regex_parses_title_first_with_index() -> None:
    chapters = _regex_tracklist("1. Intro 0:00\n2. Groove 2:15\n3. Outro 5:40")
    assert chapters is not None
    assert [(c.title, c.start) for c in chapters] == [
        ("Intro", 0),
        ("Groove", 135),
        ("Outro", 340),
    ]


def test_regex_parses_bracketed_timestamps() -> None:
    chapters = _regex_tracklist("[0:00] One\n[1:30] Two")
    assert chapters is not None
    assert [c.title for c in chapters] == ["One", "Two"]


def test_regex_ignores_prose_and_needs_two_tracks() -> None:
    assert _regex_tracklist("Just a description with no times") is None
    assert _regex_tracklist("0:00 - Only one track") is None


def test_regex_sorts_and_dedupes() -> None:
    chapters = _regex_tracklist("2:00 - B\n0:00 - A\n2:00 - dup")
    assert chapters is not None
    assert [c.title for c in chapters] == ["A", "B"]


def test_tracklist_to_chapters_from_model() -> None:
    tl = TrackList.model_validate(
        {"tracks": [{"time": "0:00", "title": "A"}, {"time": "1:00:00", "title": "B"}]}
    )
    chapters = _tracklist_to_chapters(tl)
    assert chapters is not None
    assert chapters[1].start == 3600 and chapters[1].title == "B"


def test_chapters_from_description_prefers_regex(monkeypatch) -> None:
    called = []
    monkeypatch.setattr(
        tracklist, "_agent_tracklist", lambda d, log: called.append(d) or None
    )
    chapters = chapters_from_description(REAL_DESCRIPTION)
    assert chapters is not None and len(chapters) == 6
    assert called == []  # regex succeeded, agent never consulted


def test_agent_stage_parses_cli_json(monkeypatch) -> None:
    monkeypatch.setenv("MUZIK_TAG_BACKEND", "codex")
    monkeypatch.setattr(
        tracklist,
        "_run_cli",
        lambda cmd, stdin, log: (
            '{"tracks":[{"time":"0:00","title":"X"},{"time":"1:00","title":"Y"}]}'
        ),
    )
    # A description with no regex-parseable lines forces the agent stage.
    chapters = chapters_from_description("no timestamps here that parse")
    assert chapters is not None
    assert [c.title for c in chapters] == ["X", "Y"]

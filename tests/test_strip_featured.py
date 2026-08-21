"""Tests for stripping a "feat." credit out of a track title."""

from muzik.core.chapters import strip_featured


def test_bracketed_feat() -> None:
    assert strip_featured("Song (feat. Drake)") == ("Song", ["Drake"])


def test_square_bracket_ft() -> None:
    assert strip_featured("Song [ft. Drake]") == ("Song", ["Drake"])


def test_multiple_featured_artists() -> None:
    assert strip_featured("Song (feat. A & B)") == ("Song", ["A", "B"])
    assert strip_featured("Song (feat. A, B and C)") == ("Song", ["A", "B", "C"])


def test_trailing_unbracketed_feat() -> None:
    assert strip_featured("Song feat. Drake") == ("Song", ["Drake"])
    assert strip_featured("Song featuring Drake") == ("Song", ["Drake"])


def test_no_credit_returns_title_unchanged() -> None:
    assert strip_featured("Plain Song") == ("Plain Song", [])


def test_feat_case_insensitive() -> None:
    assert strip_featured("Song (FEAT. Drake)") == ("Song", ["Drake"])

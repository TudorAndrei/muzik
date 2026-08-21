"""Tests for parsing a compilation track's "Artist - Song" title."""

from muzik.core.chapters import parse_artist_title


def test_parses_hyphen_separator() -> None:
    assert parse_artist_title("Aphex Twin - Xtal") == ("Aphex Twin", "Xtal")


def test_parses_en_and_em_dash() -> None:
    assert parse_artist_title("Boards of Canada – Roygbiv") == (
        "Boards of Canada",
        "Roygbiv",
    )
    assert parse_artist_title("Autechre — Gantz Graf") == ("Autechre", "Gantz Graf")


def test_keeps_hyphenated_names() -> None:
    # A hyphen without surrounding spaces is part of a name, not a separator.
    assert parse_artist_title("Jean-Luc Ponty") == (None, "Jean-Luc Ponty")


def test_splits_only_on_first_separator() -> None:
    # A song title that itself contains " - " keeps its remainder intact.
    assert parse_artist_title("Artist - Song - Reprise") == (
        "Artist",
        "Song - Reprise",
    )


def test_no_separator_returns_none_artist() -> None:
    assert parse_artist_title("Just A Title") == (None, "Just A Title")

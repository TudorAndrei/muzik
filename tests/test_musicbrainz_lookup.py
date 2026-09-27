"""Tests for MusicBrainz album-name cleaning and score handling."""

import logging
from types import SimpleNamespace

import pytest

from muzik.core import musicbrainz
from muzik.core.musicbrainz import clean_album_variants, get_tracklist, search_releases


def test_clean_album_variants_strips_year_and_full_album() -> None:
    # "(Full Album)" and a bracketed year go; a bare trailing year yields a
    # second variant so an album YouTube titled with a year still matches.
    assert clean_album_variants("Seeker 2023 (Full Album)") == ["Seeker 2023", "Seeker"]
    assert clean_album_variants("Rajaz (1999)") == ["Rajaz"]
    # A title without a trailing year has a single variant.
    assert clean_album_variants("Blue Waters") == ["Blue Waters"]


def test_search_releases_exposes_ext_score_as_int(monkeypatch) -> None:
    monkeypatch.setattr(
        musicbrainz, "get_native_settings", lambda: {"metadata": "beets"}
    )
    # musicbrainzngs returns the match score under "ext:score" as a string.
    monkeypatch.setattr(
        musicbrainz.musicbrainzngs,
        "search_releases",
        lambda **kwargs: {
            "release-list": [
                {"id": "r1", "title": "Seeker", "ext:score": "100"},
                {"id": "r2", "title": "Other"},  # missing score
            ]
        },
    )
    releases = search_releases("Carbon Based Lifeforms", "Seeker")
    assert releases[0]["score"] == 100
    assert releases[1]["score"] == 0  # absent score defaults to 0, not a crash


def test_native_search_and_tracklist_use_rust_results(monkeypatch) -> None:
    monkeypatch.setattr(
        musicbrainz, "get_native_settings", lambda: {"metadata": "native"}
    )
    calls = []
    native = SimpleNamespace(
        search_musicbrainz_releases=lambda *args: (
            calls.append(("search", args))
            or [{"id": "release-id", "title": "Seeker", "score": 99}]
        ),
        get_musicbrainz_tracklist=lambda release_id: (
            calls.append(("tracklist", release_id))
            or [{"title": "First", "position": 1, "length": 120000}]
        ),
    )
    monkeypatch.setattr(musicbrainz, "_load_native_module", lambda: native)
    monkeypatch.setattr(
        musicbrainz.musicbrainzngs,
        "search_releases",
        lambda **kwargs: pytest.fail("beets search ran in native mode"),
    )
    assert search_releases("Artist", "Seeker", "Unknown", 3)[0]["score"] == 99
    assert get_tracklist("release-id") == [
        {"title": "First", "position": 1, "length": 120000}
    ]
    assert calls == [
        ("search", ("Artist", "Seeker", None, 3)),
        ("tracklist", "release-id"),
    ]


def test_shadow_reports_divergence_and_returns_beets_result(
    monkeypatch, caplog
) -> None:
    monkeypatch.setattr(
        musicbrainz, "get_native_settings", lambda: {"metadata": "shadow"}
    )
    monkeypatch.setattr(
        musicbrainz.musicbrainzngs,
        "search_releases",
        lambda **kwargs: {
            "release-list": [{"id": "beets", "title": "Seeker", "ext:score": "100"}]
        },
    )
    monkeypatch.setattr(
        musicbrainz,
        "_load_native_module",
        lambda: SimpleNamespace(
            search_musicbrainz_releases=lambda *args: [
                {"id": "native", "title": "Seeker", "score": 99}
            ]
        ),
    )
    with caplog.at_level(logging.WARNING):
        releases = search_releases("Artist", "Seeker")
    assert releases[0]["id"] == "beets"
    assert "differs from beets" in caplog.text


def test_shadow_native_failure_keeps_beets_tracklist(monkeypatch, caplog) -> None:
    monkeypatch.setattr(
        musicbrainz, "get_native_settings", lambda: {"metadata": "shadow"}
    )
    monkeypatch.setattr(
        musicbrainz.musicbrainzngs,
        "get_release_by_id",
        lambda *args, **kwargs: {
            "release": {
                "medium-list": [
                    {
                        "track-list": [
                            {"title": "First", "position": "1", "length": "120000"}
                        ]
                    }
                ]
            }
        },
    )
    monkeypatch.setattr(
        musicbrainz,
        "_load_native_module",
        lambda: SimpleNamespace(
            get_musicbrainz_tracklist=lambda *args: (_ for _ in ()).throw(
                RuntimeError("offline")
            )
        ),
    )
    with caplog.at_level(logging.WARNING):
        tracks = get_tracklist("release-id")
    assert tracks == [{"title": "First", "position": 1, "length": 120000}]
    assert "native MusicBrainz track lookup failed: offline" in caplog.text

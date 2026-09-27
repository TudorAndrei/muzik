"""Tests for MusicBrainz album-name cleaning and native lookup."""

from types import SimpleNamespace

from muzik.core import musicbrainz
from muzik.core.musicbrainz import clean_album_variants, get_tracklist, search_releases


def test_clean_album_variants_strips_year_and_full_album() -> None:
    assert clean_album_variants("Seeker 2023 (Full Album)") == ["Seeker 2023", "Seeker"]
    assert clean_album_variants("Rajaz (1999)") == ["Rajaz"]
    assert clean_album_variants("Blue Waters") == ["Blue Waters"]


def test_native_search_and_tracklist_use_rust_results(monkeypatch) -> None:
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
    assert search_releases("Artist", "Seeker", "Unknown", 3)[0]["score"] == 99
    assert get_tracklist("release-id") == [
        {"title": "First", "position": 1, "length": 120000}
    ]
    assert calls == [
        ("search", ("Artist", "Seeker", None, 3)),
        ("tracklist", "release-id"),
    ]

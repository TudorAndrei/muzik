"""Tests for MusicBrainz album-name cleaning and score handling."""

from muzik.core import musicbrainz
from muzik.core.musicbrainz import clean_album_variants, search_releases


def test_clean_album_variants_strips_year_and_full_album() -> None:
    # "(Full Album)" and a bracketed year go; a bare trailing year yields a
    # second variant so an album YouTube titled with a year still matches.
    assert clean_album_variants("Seeker 2023 (Full Album)") == ["Seeker 2023", "Seeker"]
    assert clean_album_variants("Rajaz (1999)") == ["Rajaz"]
    # A title without a trailing year has a single variant.
    assert clean_album_variants("Blue Waters") == ["Blue Waters"]


def test_search_releases_exposes_ext_score_as_int(monkeypatch) -> None:
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

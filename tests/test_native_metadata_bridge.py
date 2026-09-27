"""Smoke tests for the compiled native metadata functions."""

import pytest


_native = pytest.importorskip("muzik._native")


def test_metadata_bridge_exposes_search_lookup_and_error() -> None:
    assert callable(_native.search_musicbrainz_releases)
    assert callable(_native.get_musicbrainz_tracklist)
    assert issubclass(_native.MetadataError, Exception)


def test_empty_release_fails_before_network_access() -> None:
    with pytest.raises(_native.MetadataError, match="release title is empty"):
        _native.search_musicbrainz_releases("Artist", "")

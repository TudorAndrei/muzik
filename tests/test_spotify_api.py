from collections.abc import Mapping
from email.message import Message
import time
from typing import Any, cast
import urllib.error

import pytest

from muzik.core.sources.base import ResolvedPlaylist, ResolvedTrack
from muzik.core.sources.spotify import parse_playlist_json
from muzik.core.sources.spotify_api import (
    LIKED_URI,
    SpotifyApiError,
    SpotifyClient,
    http_error,
    export_document,
    is_readable,
    playlist_reference,
)
from muzik.core.sources.spotify_auth import SpotifyTokens
import muzik.core.sources.spotify_api as spotify_api


def _tokens() -> SpotifyTokens:
    return SpotifyTokens("access-1", "refresh-1", time.time() + 3600)


def _failure(url: str, code: int, reason: str) -> urllib.error.HTTPError:
    return urllib.error.HTTPError(url, code, reason, Message(), None)


def _tracks(playlist: ResolvedPlaylist) -> list[ResolvedTrack]:
    return cast(list[ResolvedTrack], playlist.entries)


def _track(
    identifier: str,
    name: str,
    *,
    artist: str = "Kohsuke Mine",
    album: str = "Sunshower",
) -> dict[str, Any]:
    return {
        "type": "track",
        "id": identifier,
        "uri": f"spotify:track:{identifier}",
        "name": name,
        "artists": [{"name": artist}],
        "album": {
            "name": album,
            "release_date": "1976-03-01",
            "images": [{"url": f"https://i.scdn.co/image/{identifier}"}],
        },
        "duration_ms": 321000,
        "disc_number": 1,
        "track_number": 2,
        "external_ids": {"isrc": "JP1234567890"},
        "external_urls": {"spotify": f"https://open.spotify.com/track/{identifier}"},
    }


def _client(pages: dict[str, Any], **kwargs: Any) -> tuple[SpotifyClient, list[str]]:
    asked: list[str] = []

    def requester(url: str, headers: Mapping[str, str]) -> dict[str, Any]:
        asked.append(url)
        if url not in pages:
            raise http_error(_failure(url, 404, "Not Found"))
        return pages[url]

    return SpotifyClient(tokens=_tokens(), requester=requester, **kwargs), asked


def test_playlist_becomes_a_validated_export_document() -> None:
    pages = {
        "https://api.spotify.com/v1/playlists/PL1": {
            "name": "Road trip",
            "snapshot_id": "snap-1",
        },
        "https://api.spotify.com/v1/playlists/PL1/items?limit=50": {
            "items": [
                {"added_at": "2026-01-02T03:04:05Z", "item": _track("t1", "Sunshower")},
                {"item": {"type": "episode", "name": "Podcast"}},
                {"item": {"name": "Tape rip", "is_local": True, "artists": []}},
                {"item": _track("t2", "Scenery")},
            ],
            "next": None,
        },
    }
    client, _asked = _client(pages)

    playlist = client.load_playlist("spotify:playlist:PL1")

    assert playlist.title == "Road trip"
    assert playlist.source_id == "PL1"
    assert playlist.source_metadata["snapshot_id"] == "snap-1"
    first, second = _tracks(playlist)
    assert [first.title, second.title] == ["Sunshower", "Scenery"]
    assert [first.index, second.index] == [1, 2]
    assert first.artist == "Kohsuke Mine"
    assert first.album == "Sunshower"
    assert first.year == "1976"
    assert first.duration == 321.0
    assert first.source_id == "spotify:track:t1"
    assert first.source_metadata["isrc"] == "JP1234567890"
    assert first.source_metadata["image"] == "https://i.scdn.co/image/t1"
    assert first.source_metadata["added_at"] == "2026-01-02T03:04:05Z"


def test_paging_follows_the_next_url() -> None:
    pages = {
        "https://api.spotify.com/v1/me/tracks?limit=50": {
            "items": [{"track": _track("t1", "One")}],
            "next": "https://api.spotify.com/v1/me/tracks?offset=50",
        },
        "https://api.spotify.com/v1/me/tracks?offset=50": {
            "items": [{"track": _track("t2", "Two")}],
            "next": None,
        },
    }
    client, asked = _client(pages)

    playlist = client.load_playlist(LIKED_URI)

    assert playlist.title == "Liked Songs"
    assert playlist.source_id == "liked"
    assert [track.title for track in playlist.entries] == ["One", "Two"]
    assert len(asked) == 2


def test_a_renamed_endpoint_falls_back_to_the_older_name() -> None:
    pages = {
        "https://api.spotify.com/v1/playlists/PL1": {"name": "Road trip"},
        "https://api.spotify.com/v1/playlists/PL1/tracks?limit=50": {
            "items": [{"track": _track("t1", "Sunshower")}],
            "next": None,
        },
    }
    client, asked = _client(pages)

    playlist = client.load_playlist("spotify:playlist:PL1")

    assert [track.title for track in playlist.entries] == ["Sunshower"]
    assert asked[1].endswith("/items?limit=50")
    assert asked[2].endswith("/tracks?limit=50")


def test_an_expired_token_is_refreshed_once_and_the_request_repeats(
    monkeypatch,
) -> None:
    attempts: list[str] = []

    def requester(url: str, headers: Mapping[str, str]) -> dict[str, Any]:
        attempts.append(headers["Authorization"])
        if len(attempts) == 1:
            raise http_error(_failure(url, 401, "Unauthorized"))
        return {"display_name": "Tudor"}

    monkeypatch.setattr(
        spotify_api,
        "refresh_tokens",
        lambda tokens, **kwargs: SpotifyTokens(
            "access-2", "refresh-1", time.time() + 3600
        ),
    )
    client = SpotifyClient(tokens=_tokens(), requester=requester)

    assert client.account_name() == "Tudor"
    assert attempts == ["Bearer access-1", "Bearer access-2"]


def test_a_rate_limit_waits_and_repeats() -> None:
    waits: list[float] = []
    attempts: list[int] = []

    def requester(url: str, headers: Mapping[str, str]) -> dict[str, Any]:
        attempts.append(1)
        if len(attempts) == 1:
            raise _failure(url, 429, "Too Many")
        return {"display_name": "Tudor"}

    def raising(url: str, headers: Mapping[str, str]) -> dict[str, Any]:
        try:
            return requester(url, headers)
        except urllib.error.HTTPError as exc:
            raise http_error(exc) from exc

    client = SpotifyClient(tokens=_tokens(), requester=raising, sleeper=waits.append)

    assert client.account_name() == "Tudor"
    assert len(waits) == 1


def test_playlists_start_with_liked_songs() -> None:
    pages = {
        "https://api.spotify.com/v1/me/playlists?limit=50": {
            "items": [
                {
                    "id": "PL1",
                    "name": "Road trip",
                    "owner": {"display_name": "Tudor"},
                    "items": {"total": 12},
                }
            ],
            "next": None,
        }
    }
    client, _asked = _client(pages)

    liked, playlist = client.list_playlists()

    assert liked.uri == LIKED_URI
    assert liked.url == "https://open.spotify.com/collection/tracks"
    assert playlist.uri == "spotify:playlist:PL1"
    assert playlist.name == "Road trip"
    assert playlist.total == 12
    assert playlist.url == "https://open.spotify.com/playlist/PL1"


def test_a_forbidden_answer_explains_development_mode() -> None:
    def requester(url: str, headers: Mapping[str, str]) -> dict[str, Any]:
        raise http_error(_failure(url, 403, "Forbidden"))

    client = SpotifyClient(tokens=_tokens(), requester=requester)

    with pytest.raises(SpotifyApiError, match="Development Mode"):
        client.account_name()


def test_export_document_round_trips_through_the_export_parser() -> None:
    pages = {
        "https://api.spotify.com/v1/playlists/PL1": {"name": "Road trip"},
        "https://api.spotify.com/v1/playlists/PL1/items?limit=50": {
            "items": [{"item": _track("t1", "Sunshower")}],
            "next": None,
        },
    }
    client, _asked = _client(pages)
    playlist = client.load_playlist("spotify:playlist:PL1")

    again = parse_playlist_json(export_document(playlist))

    assert again.title == playlist.title
    assert [track.title for track in again.entries] == ["Sunshower"]
    assert _tracks(again)[0].source_id == "spotify:track:t1"
    assert _tracks(again)[0].duration == 321.0


def test_readable_references() -> None:
    assert is_readable("spotify:playlist:PL1") is True
    assert is_readable(LIKED_URI) is True
    assert is_readable("spotify:album:AL1") is True
    assert is_readable("spotify:track:t1") is False
    assert playlist_reference(LIKED_URI).name == "Liked Songs"

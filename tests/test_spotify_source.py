import csv
from pathlib import Path

import pytest

from muzik.core.sources.base import ResolvedTrack
from muzik.core.sources.spotify import (
    SpotifyExportError,
    is_spotify_export,
    load_playlist,
    parse_playlist_csv,
    parse_playlist_json,
)


def test_loads_synthetic_json_and_csv_fixtures() -> None:
    fixture_root = Path("tests/fixtures/spotify")

    json_playlist = load_playlist(fixture_root / "playlist_v1.json")
    csv_playlist = load_playlist(fixture_root / "exportify.csv")

    assert json_playlist.title == "Fixture playlist"
    assert csv_playlist.entries[0].source_id == "spotify:track:fixture-track"


def test_identifies_only_supported_spotify_export_files(tmp_path: Path) -> None:
    spotify_csv = tmp_path / "spotify.csv"
    spotify_csv.write_text(
        "track_name,artist_name,spotify_track_uri\nSong,Artist,spotify:track:abc\n",
        encoding="utf-8",
    )
    unrelated_csv = tmp_path / "report.csv"
    unrelated_csv.write_text("track_name,artist_name\nSong,Artist\n", encoding="utf-8")

    assert is_spotify_export(spotify_csv) is True
    assert is_spotify_export(unrelated_csv) is False


def test_parse_canonical_spotify_json_preserves_track_metadata() -> None:
    playlist = parse_playlist_json(
        {
            "version": 1,
            "source": "spotify",
            "type": "playlist",
            "id": "playlist-1",
            "title": "Road trip",
            "snapshot_id": "snapshot-1",
            "entries": [
                {
                    "index": 1,
                    "title": "Song",
                    "artists": ["Artist", "Guest"],
                    "album": "Album",
                    "release_date": "2024-02-03",
                    "duration_ms": 123000,
                    "source_id": "spotify:track:track-1",
                    "isrc": "US-ABC-24-00001",
                    "disc_number": 1,
                    "track_number": 2,
                    "added_at": "2024-02-04T05:06:07Z",
                }
            ],
        }
    )

    track = playlist.entries[0]
    assert isinstance(track, ResolvedTrack)
    assert playlist.source_id == "playlist-1"
    assert track.artist == "Artist"
    assert track.year == "2024"
    assert track.duration == 123
    assert track.source_id == "spotify:track:track-1"
    assert track.source_metadata == {
        "artists": ["Artist", "Guest"],
        "isrc": "US-ABC-24-00001",
        "disc_number": 1,
        "track_number": 2,
        "added_at": "2024-02-04T05:06:07Z",
        "local": False,
    }


def test_parse_csv_accepts_local_track_with_deterministic_id() -> None:
    rows = csv.DictReader(
        [
            "position,track_name,artist_name,album_name,duration_ms",
            "1,Local Song,Local Artist,Local Album,180000",
        ]
    )

    first = parse_playlist_csv(rows)
    second = parse_playlist_csv(
        csv.DictReader(
            [
                "position,track_name,artist_name,album_name,duration_ms",
                "1,Local Song,Local Artist,Local Album,180000",
            ]
        )
    )

    first_track = first.entries[0]
    second_track = second.entries[0]
    assert isinstance(first_track, ResolvedTrack)
    assert isinstance(second_track, ResolvedTrack)
    assert first_track.source_id == second_track.source_id
    assert first_track.source_metadata["local"] is True


@pytest.mark.parametrize(
    "entry, message",
    [
        ({"index": 1, "title": "Song"}, "requires an artist"),
        (
            {"index": 1, "title": "Episode", "artist": "Host", "type": "episode"},
            "episodes",
        ),
    ],
)
def test_parse_json_rejects_entries_without_supported_music_identity(
    entry, message
) -> None:
    payload = {
        "version": 1,
        "source": "spotify",
        "type": "playlist",
        "id": "playlist-1",
        "title": "Playlist",
        "entries": [entry],
    }

    with pytest.raises(SpotifyExportError, match=message):
        parse_playlist_json(payload)


def test_parse_json_rejects_duplicate_positions() -> None:
    payload = {
        "version": 1,
        "source": "spotify",
        "type": "playlist",
        "id": "playlist-1",
        "title": "Playlist",
        "entries": [
            {"index": 1, "title": "One", "artist": "Artist"},
            {"index": 1, "title": "Two", "artist": "Artist"},
        ],
    }

    with pytest.raises(SpotifyExportError, match="unique"):
        parse_playlist_json(payload)

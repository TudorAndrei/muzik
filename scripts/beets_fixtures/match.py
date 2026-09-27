"""Write string-distance fixtures from the installed beets release."""

from __future__ import annotations

import json
import datetime
from typing import Any
from pathlib import Path

import beets
from beets import config
from beets.autotag.distance import Distance, distance, string_dist, track_distance
from beets.autotag.hooks import AlbumInfo, TrackInfo
from beets.library import Item


CASES: tuple[tuple[str | None, str | None], ...] = (
    (None, None),
    (None, "Song"),
    ("Song", None),
    ("", ""),
    ("", "!!!"),
    ("", "Song"),
    ("Song", "Song"),
    ("SONG", "song"),
    ("A B-C!", "abc"),
    ("The Cure", "Cure, The"),
    ("A Tribe Called Quest", "Tribe Called Quest, A"),
    ("An Artist", "Artist, An"),
    ("The The", "The, The"),
    ("The Album", "Album"),
    ("The Album", "Album, The"),
    ("Simon & Garfunkel", "Simon and Garfunkel"),
    ("Rock & Roll", "Rock and Roll"),
    ("Single", ""),
    ("Song (Single)", "Song"),
    ("Song [EP]", "Song"),
    ("Song EP", "Song"),
    ("Song (feat. Guest)", "Song"),
    ("Song featuring Guest", "Song"),
    ("Song ft: Guest", "Song"),
    ("Song (live)", "Song"),
    ("Song [remaster]", "Song"),
    ("Song (live) [remaster]", "Song"),
    ("Song, pt. 2", "Song"),
    ("Song part two", "Song"),
    ("Song (feat. Guest)", "Song [live]"),
    ("Song (Acoustic)", "Song (Live)"),
    ("Beyoncé", "Beyonce"),
    ("Sigur Rós", "Sigur Ros"),
    ("Mötley Crüe", "Motley Crue"),
    ("São Paulo", "Sao Paulo"),
    ("東京", "Dong Jing"),
    ("Москва", "Moskva"),
    ("Ελλάδα", "Ellada"),
    ("Straße", "Strasse"),
    ("Æther", "Aether"),
    ("ø", "o"),
    ("🎵", ""),
    ("Line\nBreak", "Line Break"),
    ("(intro)", ""),
    ("[EP] (live)", ""),
)


TRACK_CASES: list[dict[str, Any]] = [
    {
        "name": "length within grace",
        "item": {"title": "Song", "artist": "Band", "length": 100.0},
        "track": {"title": "Song", "artist": "Band", "length": 108.0},
        "include_artist": False,
    },
    {
        "name": "length above grace",
        "item": {"title": "Song", "artist": "Band", "length": 100.0},
        "track": {"title": "Song", "artist": "Band", "length": 140.0},
        "include_artist": True,
    },
    {
        "name": "medium index accepted",
        "item": {"title": "Song", "artist": "Band", "track": 2, "disc": 2},
        "track": {"title": "Song", "index": 12, "medium_index": 2, "medium": 2},
        "include_artist": False,
    },
    {
        "name": "wrong index disc and ID",
        "item": {
            "title": "Song",
            "artist": "Band",
            "track": 3,
            "disc": 1,
            "mb_trackid": "old-id",
        },
        "track": {
            "title": "Song",
            "index": 2,
            "medium_index": 2,
            "medium": 2,
            "track_id": "new-id",
        },
        "include_artist": False,
    },
    {
        "name": "various artist local tag",
        "item": {"title": "Song", "artist": "Various Artists"},
        "track": {"title": "Song", "artist": "Solo Singer"},
        "include_artist": True,
    },
    {
        "name": "featured artist",
        "item": {"title": "Song", "artist": "Band"},
        "track": {"title": "Song", "artist": "Band feat. Guest"},
        "include_artist": True,
    },
    {
        "name": "data source mismatch",
        "item": {"title": "Song", "artist": "Band", "data_source": "Discogs"},
        "track": {"title": "Song", "data_source": "MusicBrainz"},
        "include_artist": False,
    },
]

ALBUM_CASES: list[dict[str, Any]] = [
    {
        "name": "preferred media and country",
        "items": [{"title": "One", "artist": "Band", "album": "Record", "media": "CD"}],
        "album": {
            "album": "Record",
            "artist": "Band",
            "media": "2xVinyl",
            "country": "GB",
            "tracks": [{"title": "One", "index": 1}],
        },
        "pairs": [[0, 0]],
        "preferred": {"media": ["CD", "Vinyl"], "countries": ["US", "GB"]},
    },
    {
        "name": "original year preferred",
        "items": [{"title": "One", "artist": "Band", "album": "Record"}],
        "album": {
            "album": "Record",
            "artist": "Band",
            "year": 2000,
            "original_year": 1990,
            "tracks": [{"title": "One", "index": 1}],
        },
        "pairs": [[0, 0]],
        "preferred": {"original_year": True},
    },
    {
        "name": "various artists and missing track",
        "items": [{"title": "One", "artist": "Singer", "album": "Collection"}],
        "album": {
            "album": "Collection",
            "artist": "Various Artists",
            "va": True,
            "tracks": [
                {"title": "One", "artist": "Singer", "index": 1},
                {"title": "Two", "artist": "Other", "index": 2},
            ],
        },
        "pairs": [[0, 0]],
    },
    {
        "name": "unmatched local track",
        "items": [
            {"title": "One", "artist": "Band", "album": "Record"},
            {"title": "Extra", "artist": "Band", "album": "Record"},
        ],
        "album": {
            "album": "Record",
            "artist": "Band",
            "tracks": [{"title": "One", "index": 1}],
        },
        "pairs": [[0, 0]],
    },
    {
        "name": "album ID and disc total",
        "items": [
            {
                "title": "One",
                "artist": "Band",
                "album": "Record",
                "disctotal": 1,
                "mb_albumid": "old-id",
            }
        ],
        "album": {
            "album": "Record",
            "artist": "Band",
            "album_id": "new-id",
            "mediums": 2,
            "tracks": [{"title": "One", "index": 1}],
        },
        "pairs": [[0, 0]],
    },
]


def _distance_data(result: Distance) -> dict[str, object]:
    return {
        "score": result.distance,
        "penalties": result._penalties,
    }


def _distance_cases() -> dict[str, object]:
    track_cases = []
    for case in TRACK_CASES:
        item = Item(**case["item"])
        track = TrackInfo(**case["track"])
        result = track_distance(item, track, case["include_artist"])
        track_cases.append({**case, "expected": _distance_data(result)})

    album_cases = []
    for case in ALBUM_CASES:
        preferred = case.get("preferred", {})
        for key, default in (
            ("media", []),
            ("countries", []),
            ("original_year", False),
        ):
            config["match"]["preferred"][key].set(preferred.get(key, default))
        items = [Item(**data) for data in case["items"]]
        album = AlbumInfo(
            tracks=[TrackInfo(**data) for data in case["album"]["tracks"]],
            **{key: value for key, value in case["album"].items() if key != "tracks"},
        )
        pairs = [(items[i], album.tracks[j]) for i, j in case["pairs"]]
        result = distance(items, album, pairs)
        album_cases.append(
            {
                **case,
                "expected": {
                    **_distance_data(result),
                    "tracks": [
                        _distance_data(track) for track in result.tracks.values()
                    ],
                },
            }
        )
    return {
        "beets_version": beets.__version__,
        "current_year": datetime.date.today().year,
        "tracks": track_cases,
        "albums": album_cases,
    }


def main() -> None:
    destination = (
        Path(__file__).resolve().parents[2]
        / "rust/crates/muzik-match/tests/fixtures/string_distance.json"
    )
    destination.parent.mkdir(parents=True, exist_ok=True)
    fixture = {
        "beets_version": beets.__version__,
        "cases": [
            {"left": left, "right": right, "distance": string_dist(left, right)}
            for left, right in CASES
        ],
    }
    destination.write_text(
        json.dumps(fixture, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    (destination.parent / "distance.json").write_text(
        json.dumps(_distance_cases(), ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )


if __name__ == "__main__":
    main()

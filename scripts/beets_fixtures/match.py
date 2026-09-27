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
from beets.autotag.match import (
    _add_candidate,
    _recommendation,
    _sort_candidates,
    assign_items,
)
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
        "name": "local track without length",
        "item": {"title": "Song", "artist": "Band"},
        "track": {"title": "Song", "artist": "Band", "length": 180.0},
        "include_artist": False,
    },
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

ASSIGNMENT_CASES: list[dict[str, Any]] = [
    {
        "name": "more files than tracks",
        "items": [
            {"title": "One", "artist": "Band", "track": 1},
            {"title": "Two", "artist": "Band", "track": 2},
            {"title": "Bonus", "artist": "Band", "track": 3},
        ],
        "tracks": [{"title": "One", "index": 1}, {"title": "Two", "index": 2}],
    },
    {
        "name": "more tracks than files",
        "items": [
            {"title": "One", "artist": "Band", "track": 1},
            {"title": "Three", "artist": "Band", "track": 3},
        ],
        "tracks": [
            {"title": "One", "index": 1},
            {"title": "Two", "index": 2},
            {"title": "Three", "index": 3},
        ],
    },
    {
        "name": "multiple discs",
        "items": [
            {"title": "Intro", "artist": "Band", "track": 1, "disc": 2},
            {"title": "Intro", "artist": "Band", "track": 1, "disc": 1},
        ],
        "tracks": [
            {"title": "Intro", "index": 1, "medium_index": 1, "medium": 1},
            {"title": "Intro", "index": 2, "medium_index": 1, "medium": 2},
        ],
    },
    {
        "name": "equal cost tie",
        "items": [
            {"title": "Same", "artist": "Band"},
            {"title": "Same", "artist": "Band"},
        ],
        "tracks": [{"title": "Same"}, {"title": "Same"}],
    },
]

RANKING_CASES: list[dict[str, Any]] = [
    {
        "name": "exact album ranks first",
        "items": [{"title": "One", "artist": "Band", "album": "Record"}],
        "albums": [
            {
                "album_id": "wrong",
                "album": "Different",
                "artist": "Another",
                "tracks": [{"title": "Other", "index": 1}],
            },
            {
                "album_id": "right",
                "album": "Record",
                "artist": "Band",
                "tracks": [{"title": "One", "index": 1}],
            },
        ],
    },
    {
        "name": "single distant candidate",
        "items": [{"title": "One", "artist": "Band", "album": "Record"}],
        "albums": [
            {
                "album_id": "distant",
                "album": "Other",
                "artist": "Another",
                "tracks": [{"title": "Other", "index": 1}],
            }
        ],
    },
    {
        "name": "missing track limits recommendation",
        "items": [
            {"title": f"Song {number}", "artist": "Band", "album": "Record"}
            for number in range(9)
        ],
        "albums": [
            {
                "album_id": "short",
                "album": "Record",
                "artist": "Band",
                "tracks": [
                    {"title": f"Song {number}", "index": number + 1}
                    for number in range(10)
                ],
            }
        ],
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


def _ranking_cases() -> dict[str, object]:
    config["match"]["strong_rec_thresh"].set(0.04)
    config["match"]["medium_rec_thresh"].set(0.25)
    config["match"]["rec_gap_thresh"].set(0.25)
    config["match"]["preferred"]["media"].set([])
    config["match"]["preferred"]["countries"].set([])
    config["match"]["preferred"]["original_year"].set(False)
    assignment_cases = []
    for case in ASSIGNMENT_CASES:
        items = [Item(**data) for data in case["items"]]
        tracks = [TrackInfo(**data) for data in case["tracks"]]
        pairs, extra_items, extra_tracks = assign_items(items, tracks)
        assignment_cases.append(
            {
                **case,
                "expected": {
                    "pairs": [
                        [items.index(item), tracks.index(track)]
                        for item, track in pairs
                    ],
                    "extra_items": [items.index(item) for item in extra_items],
                    "extra_tracks": [tracks.index(track) for track in extra_tracks],
                    "total_cost": sum(
                        track_distance(item, track).distance for item, track in pairs
                    ),
                },
            }
        )
    ranking_cases = []
    for case in RANKING_CASES:
        items = [Item(**data) for data in case["items"]]
        results = {}
        for data in case["albums"]:
            album = AlbumInfo(
                tracks=[TrackInfo(**track) for track in data["tracks"]],
                **{key: value for key, value in data.items() if key != "tracks"},
            )
            _add_candidate(items, results, album)
        ranked = _sort_candidates(results.values())
        ranking_cases.append(
            {
                **case,
                "expected": {
                    "ids": [match.info.album_id for match in ranked],
                    "scores": [match.distance.distance for match in ranked],
                    "recommendation": _recommendation(ranked).name,
                },
            }
        )
    return {
        "beets_version": beets.__version__,
        "assignments": assignment_cases,
        "rankings": ranking_cases,
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
    (destination.parent / "ranking.json").write_text(
        json.dumps(_ranking_cases(), ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )


if __name__ == "__main__":
    main()

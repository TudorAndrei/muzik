"""Regenerate the beets parity fixtures. Run through scripts/beets-fixtures.sh."""

import datetime
import json
import os
import re
import sys
import tempfile
import types
from importlib.metadata import version
from pathlib import Path

BEETSDIR = tempfile.TemporaryDirectory(prefix="muzik-beets-fixtures-")
os.environ["BEETSDIR"] = BEETSDIR.name

import beets
from beets import config
from beets.autotag import distance as distance_module
from beets.autotag import match as match_module
from beets.autotag.distance import distance, string_dist, track_distance
from beets.autotag.hooks import AlbumInfo, TrackInfo
from beets.library import Album, Item, Library
from beets.util import legalize_path
from mediafile import (
    ASFStorageStyle,
    MediaField,
    MediaFile,
    MP3DescStorageStyle,
    MP4StorageStyle,
    StorageStyle,
)

BEETS_VERSION = "2.13.1"
MEDIAFILE_VERSION = "0.17.0"
CURRENT_YEAR = 2026
ROOT = Path(__file__).resolve().parent.parent


class FixedDate(datetime.date):
    @classmethod
    def today(cls):
        return cls(CURRENT_YEAR, 1, 1)


def reset_config():
    config.clear()
    config.read(user=False, defaults=True)


def write(relative, data, ensure_ascii, sort_keys=False):
    path = ROOT / relative
    text = json.dumps(data, indent=2, ensure_ascii=ensure_ascii, sort_keys=sort_keys) + "\n"
    path.write_text(text, encoding="utf-8")


STRING_PAIRS = [
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
    ("AC/DC", "AC DC"),
    ("AC/DC", "ACDC"),
    ("Back\\In Black", "Back In Black"),
    ("C:Drive", "C Drive"),
    ("Song / Other", "Song - Other"),
    ("   ", ""),
    ("\t", " "),
    ("Café", "Café"),
    ("Café del Mar", "Cafe Del Mar"),
    ("Ünïcödé Tëxt", "Unicode Text"),
    ("日本語のタイトル", "Ri Ben Yu notaitoru"),
    ("한국어", "hangugeo"),
    ("Ⅻ", "XII"),
    ("ﬁnal", "final"),
    ("Song (Remix) [Live] (feat. X)", "Song"),
    ("Track 01", "Track 1"),
    ("12345", "12354"),
    ("a" * 300, "a" * 299 + "b"),
    ("Long Title " * 30, "Long Title " * 29),
    ("x" * 1000, ""),
]


def string_distance_fixture():
    return {
        "beets_version": BEETS_VERSION,
        "cases": [
            {"left": left, "right": right, "distance": string_dist(left, right)}
            for left, right in STRING_PAIRS
        ],
    }


TRACK_CASES = [
    (
        "local track without length",
        {"title": "Song", "artist": "Band"},
        {"title": "Song", "artist": "Band", "length": 180.0},
        False,
    ),
    (
        "length within grace",
        {"title": "Song", "artist": "Band", "length": 100.0},
        {"title": "Song", "artist": "Band", "length": 108.0},
        False,
    ),
    (
        "length above grace",
        {"title": "Song", "artist": "Band", "length": 100.0},
        {"title": "Song", "artist": "Band", "length": 140.0},
        True,
    ),
    (
        "medium index accepted",
        {"title": "Song", "artist": "Band", "track": 2, "disc": 2},
        {"title": "Song", "index": 12, "medium_index": 2, "medium": 2},
        False,
    ),
    (
        "wrong index disc and ID",
        {"title": "Song", "artist": "Band", "track": 3, "disc": 1, "mb_trackid": "old-id"},
        {"title": "Song", "index": 2, "medium_index": 2, "medium": 2, "track_id": "new-id"},
        False,
    ),
    (
        "various artist local tag",
        {"title": "Song", "artist": "Various Artists"},
        {"title": "Song", "artist": "Solo Singer"},
        True,
    ),
    (
        "featured artist",
        {"title": "Song", "artist": "Band"},
        {"title": "Song", "artist": "Band feat. Guest"},
        True,
    ),
    (
        "data source mismatch",
        {"title": "Song", "artist": "Band", "data_source": "Discogs"},
        {"title": "Song", "data_source": "MusicBrainz"},
        False,
    ),
    (
        "same data source",
        {"title": "Song", "artist": "Band", "data_source": "MusicBrainz"},
        {"title": "Song", "data_source": "MusicBrainz"},
        False,
    ),
    (
        "local item without data source",
        {"title": "Song", "artist": "Band"},
        {"title": "Song", "data_source": "MusicBrainz"},
        False,
    ),
    (
        "disc without track medium",
        {"title": "Song", "artist": "Band", "disc": 1},
        {"title": "Song"},
        False,
    ),
    (
        "track medium without disc",
        {"title": "Song", "artist": "Band"},
        {"title": "Song", "medium": 2},
        False,
    ),
    (
        "unicode and separators in titles",
        {"title": "Björk/Hidden Place", "artist": "Björk"},
        {"title": "Bjork - Hidden Place", "artist": "Bjork"},
        True,
    ),
]


def local_items(*titles, **fields):
    return [{"title": title, "artist": "Band", "album": "Record", **fields} for title in titles]


def release(*titles, **fields):
    return {
        "album": "Record",
        "artist": "Band",
        **fields,
        "tracks": [{"title": title, "index": index + 1} for index, title in enumerate(titles)],
    }


ALBUM_CASES = [
    (
        "preferred media and country",
        [{"title": "One", "artist": "Band", "album": "Record", "media": "CD"}],
        {
            "album": "Record",
            "artist": "Band",
            "media": "2xVinyl",
            "country": "GB",
            "tracks": [{"title": "One", "index": 1}],
        },
        [[0, 0]],
        {"media": ["CD", "Vinyl"], "countries": ["US", "GB"]},
    ),
    (
        "original year preferred",
        [{"title": "One", "artist": "Band", "album": "Record"}],
        {
            "album": "Record",
            "artist": "Band",
            "year": 2000,
            "original_year": 1990,
            "tracks": [{"title": "One", "index": 1}],
        },
        [[0, 0]],
        {"original_year": True},
    ),
    (
        "various artists and missing track",
        [{"title": "One", "artist": "Singer", "album": "Collection"}],
        {
            "album": "Collection",
            "artist": "Various Artists",
            "va": True,
            "tracks": [
                {"title": "One", "artist": "Singer", "index": 1},
                {"title": "Two", "artist": "Other", "index": 2},
            ],
        },
        [[0, 0]],
        None,
    ),
    (
        "unmatched local track",
        [
            {"title": "One", "artist": "Band", "album": "Record"},
            {"title": "Extra", "artist": "Band", "album": "Record"},
        ],
        {"album": "Record", "artist": "Band", "tracks": [{"title": "One", "index": 1}]},
        [[0, 0]],
        None,
    ),
    (
        "album ID and disc total",
        [
            {
                "title": "One",
                "artist": "Band",
                "album": "Record",
                "disctotal": 1,
                "mb_albumid": "old-id",
            }
        ],
        {
            "album": "Record",
            "artist": "Band",
            "album_id": "new-id",
            "mediums": 2,
            "tracks": [{"title": "One", "index": 1}],
        },
        [[0, 0]],
        None,
    ),
    (
        "plurality tie keeps the first value",
        [
            {"title": "One", "artist": "Band", "album": "Record"},
            {"title": "Two", "artist": "Other", "album": "Different"},
        ],
        release("One", "Two"),
        [[0, 0], [1, 1]],
        None,
    ),
    (
        "album artist consensus",
        [
            {"title": "One", "artist": "Singer", "albumartist": "Band", "album": "Record"},
            {"title": "Two", "artist": "Guest", "albumartist": "Band", "album": "Record"},
        ],
        release("One", "Two"),
        [[0, 0], [1, 1]],
        None,
    ),
    ("current media matches", local_items("One", media="CD"), release("One", media="CD"), [[0, 0]], None),
    (
        "current media differs",
        local_items("One", media="CD"),
        release("One", media="Vinyl"),
        [[0, 0]],
        None,
    ),
    (
        "preferred media as a string",
        local_items("One"),
        release("One", media="Vinyl"),
        [[0, 0]],
        {"media": "CD Vinyl"},
    ),
    ("same year", local_items("One", year=2000), release("One", year=2000), [[0, 0]], None),
    (
        "original year matches",
        local_items("One", year=1990),
        release("One", year=2000, original_year=1990),
        [[0, 0]],
        None,
    ),
    (
        "year between releases",
        local_items("One", year=1995),
        release("One", year=2000, original_year=1990),
        [[0, 0]],
        None,
    ),
    (
        "year without original",
        local_items("One", year=1995),
        release("One", year=2000),
        [[0, 0]],
        None,
    ),
    (
        "current country",
        local_items("One", country="GB"),
        release("One", country="DE"),
        [[0, 0]],
        None,
    ),
    (
        "label catalog and disambiguation",
        local_items("One", label="Harbor", catalognum="HR-1", albumdisambig="remaster"),
        release("One", label="Harbor Records", catalognum="HR-1", albumdisambig="deluxe"),
        [[0, 0]],
        None,
    ),
    (
        "label only on the candidate",
        local_items("One"),
        release("One", label="Harbor", catalognum="HR-1", albumdisambig="deluxe"),
        [[0, 0]],
        None,
    ),
    (
        "unicode album names",
        local_items("Hidden Place", album="Vespertine/Live"),
        release("Hidden Place", album="Vespertine – Live"),
        [[0, 0]],
        None,
    ),
]


def make_album(fields):
    fields = dict(fields)
    tracks = [TrackInfo(**track) for track in fields.pop("tracks")]
    return AlbumInfo(tracks=tracks, **fields)


def expected_distance(dist, with_tracks):
    expected = {"score": dist.distance, "penalties": dict(dist._penalties)}
    if with_tracks:
        expected["tracks"] = [
            expected_distance(track, False) for track in dist.tracks.values()
        ]
    return expected


def set_preferred(preferred):
    config["match"]["preferred"].set(
        {"countries": [], "media": [], "original_year": False, **(preferred or {})}
    )


def distance_fixture():
    distance_module.datetime = types.SimpleNamespace(date=FixedDate)
    tracks = []
    for name, item, track, include_artist in TRACK_CASES:
        dist = track_distance(Item(**item), TrackInfo(**track), include_artist)
        tracks.append(
            {
                "name": name,
                "item": item,
                "track": track,
                "include_artist": include_artist,
                "expected": expected_distance(dist, False),
            }
        )
    albums = []
    for name, items, album, pairs, preferred in ALBUM_CASES:
        reset_config()
        set_preferred(preferred)
        beets_items = [Item(**item) for item in items]
        info = make_album(album)
        dist = distance(
            beets_items,
            info,
            [(beets_items[left], info.tracks[right]) for left, right in pairs],
        )
        case = {"name": name, "items": items, "album": album, "pairs": pairs}
        if preferred is not None:
            case["preferred"] = preferred
        case["expected"] = expected_distance(dist, True)
        albums.append(case)
    reset_config()
    return {
        "beets_version": BEETS_VERSION,
        "current_year": CURRENT_YEAR,
        "tracks": tracks,
        "albums": albums,
    }


ASSIGNMENT_CASES = [
    (
        "more files than tracks",
        [
            {"title": "One", "artist": "Band", "track": 1},
            {"title": "Two", "artist": "Band", "track": 2},
            {"title": "Bonus", "artist": "Band", "track": 3},
        ],
        [{"title": "One", "index": 1}, {"title": "Two", "index": 2}],
    ),
    (
        "more tracks than files",
        [
            {"title": "One", "artist": "Band", "track": 1},
            {"title": "Three", "artist": "Band", "track": 3},
        ],
        [
            {"title": "One", "index": 1},
            {"title": "Two", "index": 2},
            {"title": "Three", "index": 3},
        ],
    ),
    (
        "multiple discs",
        [
            {"title": "Intro", "artist": "Band", "track": 1, "disc": 2},
            {"title": "Intro", "artist": "Band", "track": 1, "disc": 1},
        ],
        [
            {"title": "Intro", "index": 1, "medium_index": 1, "medium": 1},
            {"title": "Intro", "index": 2, "medium_index": 1, "medium": 2},
        ],
    ),
    (
        "equal cost tie",
        [{"title": "Same", "artist": "Band"}, {"title": "Same", "artist": "Band"}],
        [{"title": "Same"}, {"title": "Same"}],
    ),
]

RANKING_CASES = [
    (
        "exact album ranks first",
        [{"title": "One", "artist": "Band", "album": "Record"}],
        [
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
    ),
    (
        "single distant candidate",
        [{"title": "One", "artist": "Band", "album": "Record"}],
        [
            {
                "album_id": "distant",
                "album": "Other",
                "artist": "Another",
                "tracks": [{"title": "Other", "index": 1}],
            }
        ],
    ),
    (
        "missing track limits recommendation",
        [{"title": f"Song {index}", "artist": "Band", "album": "Record"} for index in range(9)],
        [
            {
                "album_id": "short",
                "album": "Record",
                "artist": "Band",
                "tracks": [
                    {"title": f"Song {index}", "index": index + 1} for index in range(10)
                ],
            }
        ],
    ),
    (
        "required fields drop candidates",
        local_items("One"),
        [
            {**release("One", year=2000), "album_id": "dated"},
            {**release("One"), "album_id": "undated"},
        ],
        {"required": ["album", "artist", "year"]},
    ),
    (
        "ignored penalty drops a candidate",
        local_items("One", mb_albumid="local-id"),
        [
            {**release("One"), "album_id": "other-id"},
            {**release("One"), "album_id": "local-id"},
        ],
        {"ignored": ["album_id"]},
    ),
    (
        "ignored zero penalty keeps a candidate",
        local_items("One"),
        [{**release("One"), "album_id": "exact"}],
        {"ignored": ["artist"]},
    ),
    (
        "max rec ignores zero penalties",
        local_items("One"),
        [{**release("One"), "album_id": "exact"}],
        {"max_rec": {"artist": "low"}},
    ),
    (
        "max rec limits a track penalty",
        local_items(*[f"Song {index}" for index in range(9)], "Xxxx"),
        [
            {
                **release(*[f"Song {index}" for index in range(9)], "Zzzz"),
                "album_id": "one-title-differs",
            }
        ],
        {"max_rec": {"track_title": "low"}},
    ),
    (
        "strong threshold is exclusive",
        local_items("One"),
        [{**release("One"), "album_id": "exact"}],
        {"strong_rec_thresh": 0.0},
    ),
    (
        "large gap gives a low recommendation",
        local_items("One"),
        [
            {**release("One", album="Other"), "album_id": "near"},
            {**release("Two", album="Other", artist="Someone"), "album_id": "far"},
        ],
    ),
    (
        "small gap gives no recommendation",
        local_items("One"),
        [
            {**release("One", album="Other"), "album_id": "near"},
            {**release("Ona", album="Other"), "album_id": "close"},
        ],
    ),
]


def position(values, target):
    return next(index for index, value in enumerate(values) if value is target)


def ranking_fixture():
    assignments = []
    for name, items, tracks in ASSIGNMENT_CASES:
        beets_items = [Item(**item) for item in items]
        infos = [TrackInfo(**track) for track in tracks]
        pairs, extra_items, extra_tracks = match_module.assign_items(beets_items, infos)
        assignments.append(
            {
                "name": name,
                "items": items,
                "tracks": tracks,
                "expected": {
                    "pairs": [
                        [beets_items.index(item), infos.index(track)]
                        for item, track in pairs
                    ],
                    "extra_items": [position(beets_items, item) for item in extra_items],
                    "extra_tracks": [position(infos, track) for track in extra_tracks],
                    "total_cost": sum(
                        track_distance(item, track).distance for item, track in pairs
                    ),
                },
            }
        )
    rankings = []
    for name, items, albums, *overrides in RANKING_CASES:
        reset_config()
        for key, value in (overrides[0] if overrides else {}).items():
            config["match"][key].set(value)
        beets_items = [Item(**item) for item in items]
        candidates = {}
        for album in albums:
            match_module._add_candidate(beets_items, candidates, make_album(album))
        ranked = match_module._sort_candidates(candidates.values())
        case = {"name": name, "items": items, "albums": albums}
        if overrides:
            case["config"] = overrides[0]
        rankings.append(
            {
                **case,
                "expected": {
                    "ids": [candidate.info.album_id for candidate in ranked],
                    "scores": [candidate.distance.distance for candidate in ranked],
                    "recommendation": match_module._recommendation(ranked).name,
                },
            }
        )
    reset_config()
    return {
        "beets_version": BEETS_VERSION,
        "assignments": assignments,
        "rankings": rankings,
    }


PATH_FORMATS = [
    ("default", "$albumartist/$album%aunique{}/$track $title"),
    ("comp:true", "Compilations/$album%aunique{}/$track $title"),
    ("singleton:true", "Non-Album/$artist/$title"),
]

PATH_FIELDS = ["albumartist", "artist", "album", "title", "track", "year", "label"]


def album_track(albumartist, album, title, track, year=0, artist=None, label=""):
    return {
        "albumartist": albumartist,
        "artist": albumartist if artist is None else artist,
        "album": album,
        "title": title,
        "track": track,
        "year": year,
        "label": label,
    }


PATH_CASES = [
    ("first", "default", album_track("Portishead", "Dummy", "Mysterons", 1, 1994)),
    ("second", "default", album_track("Portishead", "Dummy", "Sour Times", 2, 2008)),
    (
        "compilation",
        "comp",
        album_track("Various Artists", "Sampler", "Hidden Place", 1, artist="Björk"),
    ),
    ("singleton", "singleton", {"artist": "Björk", "title": "Hidden Place"}),
    (
        "separators in fields",
        "default",
        album_track("AC/DC", "Live/Wire", "Back\\In/Black", 1, 1980),
    ),
    (
        "drive letters in fields",
        "default",
        album_track("C:Drive", "d:/Side A:B", "E:F:G", 2, 1990),
    ),
    (
        "compilation separators",
        "comp",
        album_track("Various Artists", "Now/Then", "Track/One", 3, artist="X/Y"),
    ),
    ("singleton separators", "singleton", {"artist": "Simon/Garfunkel", "title": "A/B\\C"}),
    (
        "disambiguator with separator",
        "default",
        album_track("Separator Band", "Same Name", "Song", 1, 2000, label="Left/Right"),
    ),
    (
        "second disambiguator with separator",
        "default",
        album_track("Separator Band", "Same Name", "Song", 1, 2000, label="Up/Down"),
    ),
    (
        "only separators",
        "default",
        album_track("/", "//", "/ /", 4, 2001),
    ),
    (
        "long names",
        "default",
        album_track("Long Artist", "A" * 300, "T" * 300, 5, 2002),
    ),
    (
        "long multibyte names",
        "default",
        album_track("Long Artist", "é" * 200, "ö" * 200, 6, 2003),
    ),
]

TEMPLATE_CASE = "first"

TEMPLATES = [
    "%left{$title,4}",
    "%right{$title,4}",
    "%lower{$albumartist}",
    "%upper{$album}",
    "%title{$title}",
    "%asciify{Björk Guðmundsdóttir}",
    "%if{1,yes,no}",
    "%if{0,yes,no}",
    "%if{false,yes,no}",
    "%if{,yes,no}",
    "%if{$track,a$,b,c}",
    "%lower{A$}B}",
    "$$title ${title}",
    "${}",
    "%aunique{albumartist album,year,()}",
    "%aunique{albumartist album,label,[]}",
    "%aunique{album,year}",
]

SANITIZATION_CASES = [
    ("Artist/Bad: title?", ".FLAC", None),
    (".Hidden/ -track. ", ".mp3", None),
    ("Artist/Name\\slash", ".ogg", None),
    ("\x01" + "e" * 300 + "/x", ".flac", [["e", "ee"]]),
    ("\x01" + "e" * 199 + "/x", ".flac", [["z", "z"]]),
]


def paths_fixture():
    reset_config()
    library = Library(Path(os.environ["BEETSDIR"]) / "library.db", "/music")
    library.path_formats = PATH_FORMATS
    stored = []
    for name, path_format, fields in PATH_CASES:
        item = Item(path=b"/source/track.flac", **fields)
        if path_format == "comp":
            item.comp = True
        if path_format == "singleton":
            library.add(item)
            stored.append((name, path_format, item, None))
        else:
            album = library.add_album([item])
            stored.append((name, path_format, item, album))
    cases = []
    for name, path_format, item, album in stored:
        cases.append(
            {
                "name": name,
                "path_format": path_format,
                "album_id": album.id if album else None,
                "fields": {
                    key: item._type(key).format(item.get(key)) for key in PATH_FIELDS
                },
                "destination": item.destination(relative_to_libdir=True).decode(),
            }
        )
    sanitization = []
    for subpath, extension, replacements in SANITIZATION_CASES:
        case = {"subpath": subpath, "extension": extension}
        if replacements is None:
            rules = library.replacements
        else:
            case["replacements"] = replacements
            rules = [(re.compile(pattern), value) for pattern, value in replacements]
        case["destination"] = legalize_path(subpath, rules, extension)[0]
        sanitization.append(case)
    item = next(item for name, _, item, _ in stored if name == TEMPLATE_CASE)
    templates = [
        {
            "case": TEMPLATE_CASE,
            "template": template,
            "rendered": item.evaluate_template(template, for_path=True),
        }
        for template in TEMPLATES
    ]
    return {
        "beets_version": BEETS_VERSION,
        "cases": cases,
        "sanitization": sanitization,
        "templates": templates,
    }


TAG_SUFFIXES = ["flac", "m4a", "mp3", "ogg", "opus"]

TAG_FIELDS = [
    "album",
    "albumartist",
    "albumartists",
    "albumdisambig",
    "artist",
    "artists",
    "catalognum",
    "comp",
    "country",
    "day",
    "disc",
    "disctotal",
    "label",
    "mb_albumartistid",
    "mb_albumid",
    "mb_artistid",
    "mb_releasegroupid",
    "mb_releasetrackid",
    "mb_trackid",
    "mb_workid",
    "media",
    "month",
    "muzik_mood",
    "original_day",
    "original_month",
    "original_year",
    "rg_album_gain",
    "rg_album_peak",
    "rg_track_gain",
    "rg_track_peak",
    "title",
    "track",
    "tracktotal",
    "year",
]


def tags_fixture():
    MediaFile.add_field(
        "muzik_mood",
        MediaField(
            MP3DescStorageStyle("MUZIK_MOOD"),
            MP4StorageStyle("----:com.apple.iTunes:MUZIK_MOOD"),
            StorageStyle("MUZIK_MOOD"),
            ASFStorageStyle("MUZIK_MOOD"),
        ),
    )
    directory = ROOT / "crates/muzik-tags/tests/fixtures"
    files = {}
    for suffix in TAG_SUFFIXES:
        media = MediaFile(directory / f"mediafile.{suffix}")
        files[suffix] = {field: getattr(media, field) for field in TAG_FIELDS}
    return {
        "beets_version": BEETS_VERSION,
        "files": files,
        "mediafile_version": MEDIAFILE_VERSION,
    }


def main():
    if beets.__version__ != BEETS_VERSION:
        sys.exit(f"expected beets {BEETS_VERSION}, found {beets.__version__}")
    if version("mediafile") != MEDIAFILE_VERSION:
        sys.exit(f"expected mediafile {MEDIAFILE_VERSION}, found {version('mediafile')}")
    os.chdir(BEETSDIR.name)
    reset_config()
    write(
        "crates/muzik-match/tests/fixtures/string_distance.json",
        string_distance_fixture(),
        False,
    )
    write("crates/muzik-match/tests/fixtures/distance.json", distance_fixture(), False)
    write("crates/muzik-match/tests/fixtures/ranking.json", ranking_fixture(), False)
    write("crates/muzik-import/tests/fixtures/paths.json", paths_fixture(), True)
    write(
        "crates/muzik-tags/tests/fixtures/mediafile_tags.json",
        tags_fixture(),
        True,
        sort_keys=True,
    )


if __name__ == "__main__":
    try:
        main()
    finally:
        BEETSDIR.cleanup()

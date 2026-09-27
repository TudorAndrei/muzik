"""Convert live beets album tasks for the native Rust matcher."""

from __future__ import annotations

from dataclasses import dataclass
import json
from typing import Any

from beets import config as beets_config
from beets import metadata_plugins

from muzik import _native


@dataclass(frozen=True, slots=True)
class RankedCandidate:
    original_index: int
    distance: float


@dataclass(frozen=True, slots=True)
class NativeRanking:
    candidates: list[RankedCandidate]
    recommendation: str


_ITEM_FIELDS = (
    "title",
    "artist",
    "album",
    "albumartist",
    "length",
    "track",
    "disc",
    "disctotal",
    "year",
    "media",
    "country",
    "label",
    "catalognum",
    "albumdisambig",
    "mb_albumid",
    "mb_trackid",
    "data_source",
    "comp",
)
_TRACK_FIELDS = (
    "title",
    "artist",
    "length",
    "index",
    "medium",
    "medium_index",
    "track_id",
    "data_source",
)
_ALBUM_FIELDS = (
    "album",
    "artist",
    "album_id",
    "va",
    "media",
    "mediums",
    "year",
    "original_year",
    "country",
    "label",
    "catalognum",
    "albumdisambig",
    "data_source",
)


def _field(value: Any, name: str) -> Any:
    if isinstance(value, dict):
        return value.get(name)
    result = getattr(value, name, None)
    if result is None and hasattr(value, "get"):
        result = value.get(name)
    return result


def _fields(value: Any, names: tuple[str, ...]) -> dict[str, Any]:
    result = {}
    for name in names:
        field = _field(value, name)
        if field is not None:
            result[name] = field
    return result


def _match_config() -> dict[str, Any]:
    match = beets_config["match"]
    weights = match["distance_weights"]
    max_rec = match["max_rec"]
    sources = metadata_plugins.find_metadata_source_plugins()
    return {
        "match": {
            "distance_weights": {
                key: weights[key].as_number() for key in weights.keys()
            },
            "preferred": {
                "media": match["preferred"]["media"].as_str_seq(),
                "countries": match["preferred"]["countries"].as_str_seq(),
                "original_year": match["preferred"]["original_year"].get(bool),
            },
            "track_length_grace": match["track_length_grace"].as_number(),
            "track_length_max": match["track_length_max"].as_number(),
            "strong_rec_thresh": match["strong_rec_thresh"].as_number(),
            "medium_rec_thresh": match["medium_rec_thresh"].as_number(),
            "rec_gap_thresh": match["rec_gap_thresh"].as_number(),
            "max_rec": {key: max_rec[key].get(str) for key in max_rec.keys()},
            "ignored": match["ignored"].as_str_seq(),
            "required": match["required"].as_str_seq(),
        },
        "metadata_source_count": len(sources),
        "data_source_penalties": {
            source.data_source: source.data_source_mismatch_penalty
            for source in sources
        },
    }


def rank_album_candidates(task: Any) -> NativeRanking:
    """Return native distance and order for the existing beets candidates."""
    items = [_fields(item, _ITEM_FIELDS) for item in task.items]
    albums = []
    for candidate in task.candidates:
        info = candidate.info
        album = _fields(info, _ALBUM_FIELDS)
        album["tracks"] = [_fields(track, _TRACK_FIELDS) for track in info.tracks]
        albums.append(album)
    rows, recommendation = _native.rank_album_candidates(
        json.dumps(items), json.dumps(albums), json.dumps(_match_config())
    )
    candidates = [
        RankedCandidate(int(index), float(distance)) for index, distance in rows
    ]
    if len({row.original_index for row in candidates}) != len(candidates):
        raise ValueError("native matcher returned duplicate candidate indices")
    if any(not 0 <= row.original_index < len(albums) for row in candidates):
        raise ValueError("native matcher returned an invalid candidate index")
    return NativeRanking(candidates, str(recommendation))

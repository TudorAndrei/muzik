"""Metadata-only Spotify playlist export parsing.

This module never calls Spotify or downloads Spotify media.
"""

from __future__ import annotations

import csv
import hashlib
import json
from pathlib import Path
from typing import Any, cast

from muzik.core.sources.base import ResolvedPlaylist, ResolvedTrack


class SpotifyExportError(ValueError):
    """Raised when a Spotify metadata export is unsupported or malformed."""


def load_playlist(path: Path) -> ResolvedPlaylist:
    """Load the explicitly supported JSON or CSV export at *path*."""
    suffix = path.suffix.lower()
    if suffix == ".json":
        return parse_playlist_json(json.loads(path.read_text(encoding="utf-8")))
    if suffix == ".csv":
        with path.open(newline="", encoding="utf-8") as handle:
            return parse_playlist_csv(csv.DictReader(handle))
    raise SpotifyExportError("Spotify exports must be JSON or CSV files.")


def is_spotify_export(path: Path) -> bool:
    """Return whether *path* is a supported Spotify metadata export.

    This deliberately performs only format identification.  A malformed file
    that identifies itself as Spotify is still handed to ``load_playlist`` so
    callers can report the actionable validation error instead of treating it
    as an audio input.
    """
    if not path.is_file():
        return False
    if path.suffix.lower() == ".json":
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
        except OSError, json.JSONDecodeError:
            return False
        return isinstance(data, dict) and data.get("source") == "spotify"
    if path.suffix.lower() != ".csv":
        return False
    try:
        with path.open(newline="", encoding="utf-8") as handle:
            fields = {
                name.strip().lower() for name in csv.DictReader(handle).fieldnames or []
            }
    except OSError:
        return False
    required = {"track_name", "artist_name"}
    spotify_markers = {
        "spotify_track_id",
        "spotify_track_uri",
        "spotify_track_url",
        "isrc",
    }
    return required <= fields and bool(fields & spotify_markers)


def parse_playlist_json(data: Any) -> ResolvedPlaylist:
    if not isinstance(data, dict):
        raise SpotifyExportError("Spotify JSON export must be an object.")
    if data.get("version") != 1 or data.get("source") != "spotify":
        raise SpotifyExportError(
            "Spotify JSON export requires version 1 and source spotify."
        )
    if data.get("type") != "playlist":
        raise SpotifyExportError("Spotify JSON export type must be playlist.")
    playlist_id = _required(data, "id", "playlist")
    title = _required(data, "title", "playlist")
    entries = data.get("entries")
    if not isinstance(entries, list):
        raise SpotifyExportError("Spotify JSON export requires an entries array.")
    tracks = [_track_from_json(item, index) for index, item in enumerate(entries, 1)]
    _validate_positions(tracks)
    return ResolvedPlaylist(
        title=title,
        entries=cast(list, tracks),
        source="spotify",
        source_id=playlist_id,
        source_url=_optional_string(data.get("source_url")),
        source_metadata={"snapshot_id": data.get("snapshot_id"), "version": 1},
    )


def parse_playlist_csv(rows: csv.DictReader[str]) -> ResolvedPlaylist:
    fieldnames = {name.strip().lower() for name in rows.fieldnames or []}
    required = {"track_name", "artist_name"}
    if not required <= fieldnames:
        raise SpotifyExportError(
            "Spotify CSV requires track_name and artist_name columns."
        )
    tracks: list[ResolvedTrack] = []
    playlist_title = "Spotify CSV import"
    for line, row in enumerate(rows, 2):
        normalized = {
            key.strip().lower(): (value or "").strip()
            for key, value in row.items()
            if key
        }
        if normalized.get("episode") or normalized.get("type", "").lower() == "episode":
            raise SpotifyExportError(
                f"Spotify CSV contains an unsupported episode at row {line}."
            )
        title = normalized.get("track_name", "")
        artist = normalized.get("artist_name", "")
        if not title or not artist:
            raise SpotifyExportError(
                f"Spotify CSV row {line} requires track_name and artist_name."
            )
        position = _integer(normalized.get("position"), default=line - 1)
        track_id = normalized.get("spotify_track_id") or _id_from_uri(
            normalized.get("spotify_track_uri")
        )
        source_id = (
            f"spotify:track:{track_id}"
            if track_id
            else _local_source_id(title, artist, position)
        )
        tracks.append(
            ResolvedTrack(
                title=title,
                artist=artist,
                album=normalized.get("album_name") or None,
                year=_year(normalized.get("release_date")),
                index=position,
                duration=_duration_seconds(normalized.get("duration_ms")),
                source="spotify",
                source_id=source_id,
                source_url=normalized.get("spotify_track_url") or None,
                source_metadata={
                    "artists": _artists(normalized.get("artist_names") or artist),
                    "isrc": normalized.get("isrc") or None,
                    "disc_number": _integer(normalized.get("disc_number")),
                    "track_number": _integer(normalized.get("album_track_number")),
                    "added_at": normalized.get("added_at") or None,
                    "local": not bool(track_id),
                },
            )
        )
    if not tracks:
        raise SpotifyExportError("Spotify CSV contains no music tracks.")
    _validate_positions(tracks)
    return ResolvedPlaylist(
        title=playlist_title,
        entries=cast(list, tracks),
        source="spotify",
        source_id=_local_source_id(playlist_title, "spotify", 0),
    )


def _track_from_json(item: Any, fallback_position: int) -> ResolvedTrack:
    if not isinstance(item, dict):
        raise SpotifyExportError("Spotify playlist entries must be objects.")
    if item.get("type") == "episode":
        raise SpotifyExportError(
            "Spotify exports containing episodes are not supported."
        )
    title = _required(item, "title", "track")
    artists = item.get("artists")
    artist = _optional_string(item.get("artist")) or (
        artists[0] if isinstance(artists, list) and artists else None
    )
    if not artist:
        raise SpotifyExportError(f"Spotify track {title!r} requires an artist.")
    position = _integer(
        item.get("index") or item.get("position"), default=fallback_position
    )
    source_id = _optional_string(item.get("source_id"))
    if not source_id:
        source_id = _local_source_id(title, artist, position)
    source_metadata = dict(item.get("source_metadata") or {})
    source_metadata.update(
        {
            key: value
            for key, value in {
                "artists": artists if isinstance(artists, list) else None,
                "isrc": _optional_string(item.get("isrc")),
                "disc_number": _integer(item.get("disc_number")),
                "track_number": _integer(item.get("track_number")),
                "added_at": _optional_string(item.get("added_at")),
                "local": not bool(_optional_string(item.get("source_id"))),
            }.items()
            if value is not None
        }
    )
    return ResolvedTrack(
        title=title,
        artist=artist,
        album=_optional_string(item.get("album")),
        year=_year(item.get("year") or item.get("release_date")),
        index=position,
        duration=_duration_seconds(item.get("duration") or item.get("duration_ms")),
        source="spotify",
        source_id=source_id,
        source_url=_optional_string(item.get("source_url")),
        source_metadata=source_metadata,
    )


def _required(data: dict[str, Any], key: str, kind: str) -> str:
    value = _optional_string(data.get(key))
    if not value:
        raise SpotifyExportError(f"Spotify {kind} requires {key}.")
    return value


def _optional_string(value: Any) -> str | None:
    return str(value).strip() if value is not None and str(value).strip() else None


def _integer(value: Any, default: int | None = None) -> int | None:
    if value in (None, ""):
        return default
    try:
        return int(str(value))
    except ValueError as exc:
        raise SpotifyExportError(f"Expected an integer, got {value!r}.") from exc


def _duration_seconds(value: Any) -> float | None:
    if value in (None, ""):
        return None
    try:
        duration = float(str(value))
    except ValueError as exc:
        raise SpotifyExportError(f"Expected a duration, got {value!r}.") from exc
    return duration / 1000 if duration > 1000 else duration


def _year(value: Any) -> str | None:
    text = _optional_string(value)
    return text[:4] if text else None


def _id_from_uri(uri: str | None) -> str | None:
    if not uri:
        return None
    return uri.rsplit(":", 1)[-1] if ":" in uri else uri


def _artists(value: str) -> list[str]:
    return [artist.strip() for artist in value.split(",") if artist.strip()]


def _local_source_id(title: str, artist: str, position: int | None) -> str:
    digest = hashlib.sha256(f"{title}\0{artist}\0{position}".encode()).hexdigest()[:16]
    return f"spotify:local:{digest}"


def _validate_positions(tracks: list[ResolvedTrack]) -> None:
    positions = [track.index for track in tracks]
    if any(position is None or position < 1 for position in positions):
        raise SpotifyExportError("Spotify track positions must be positive integers.")
    if len(set(positions)) != len(positions):
        raise SpotifyExportError("Spotify track positions must be unique.")

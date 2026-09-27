"""Audio probe and metadata helpers."""

import json
import re
from pathlib import Path
from typing import Optional

from muzik.core.chapters import sidecar_path
from muzik.core.metadata import find_muzik_metadata
from muzik.core.musicbrainz import clean_album_name


def _parse_title(title: str) -> tuple[str, str, str]:
    """Best-effort parse of ``"Artist - Album (Year)"`` YouTube title patterns.

    Returns ``(artist, album, year)`` — any part may be empty string.
    """
    artist = album = year = ""
    # Strip trailing year like " (1998)" or " [2004]"
    year_match = re.search(r"[\(\[]((?:19|20)\d{2})[\)\]]", title)
    if year_match:
        year = year_match.group(1)
        title = title[: year_match.start()].rstrip()

    if " - " in title:
        parts = title.split(" - ", 1)
        artist = parts[0].strip()
        album = parts[1].strip()
    else:
        album = title.strip()
    return artist, clean_album_name(album), year


def _probe_native(path: Path) -> dict:
    from muzik import _native

    data = _native.probe_audio(str(path))
    tags = data["tags"]
    duration = data["duration"]
    bitrate = data["bitrate_kbps"]
    stream = {
        "codec_type": "audio",
        "codec_name": data["codec"],
        "sample_rate": data["sample_rate_hz"],
        "channels": data["channels"],
        "bit_rate": bitrate * 1000 if bitrate is not None else None,
        "bits_per_raw_sample": data["bit_depth"],
        "duration": duration,
        "tags": tags,
    }
    return {
        "format": {
            "duration": duration,
            "bit_rate": stream["bit_rate"],
            "size": data["size_bytes"],
            "tags": tags,
        },
        "streams": [stream],
        "chapters": [],
    }


def probe(path: Path) -> dict:
    """Probe an audio file with the native tag reader."""
    try:
        return _probe_native(path)
    except Exception as exc:
        raise ValueError(f"native audio probe failed for {path}: {exc}") from exc


def get_duration(path: Path) -> Optional[float]:
    """Return audio duration in seconds, or None on failure."""
    try:
        data = probe(path)
        return float(data["format"]["duration"])
    except KeyError, ValueError, TypeError:
        return None


def extract_metadata(path: Path) -> dict:
    """Return a dict with keys: title, artist, album, year.

    Preference order:
    1. Source-neutral .muzik.json metadata
    2. Sidecar .info.json (yt-dlp metadata)
    3. Embedded tags
    4. Reasonable fallbacks
    """
    muzik_meta = find_muzik_metadata(path)
    if muzik_meta:
        resolved = muzik_meta.get("resolved") or {}
        if not isinstance(resolved, dict):
            resolved = {}
        candidate = muzik_meta.get("candidate") or {}
        if not isinstance(candidate, dict):
            candidate = {}
        candidate_metadata = candidate.get("metadata") or {}
        if not isinstance(candidate_metadata, dict):
            candidate_metadata = {}
        sources = (resolved, candidate_metadata, muzik_meta, candidate)
        if any(
            source.get(field)
            for source in sources
            for field in ("title", "track", "artist", "album", "year")
        ):
            title = next(
                (
                    value
                    for source in sources
                    for key in ("title", "track")
                    if (value := source.get(key))
                ),
                path.stem,
            )
            artist = next(
                (source["artist"] for source in sources if source.get("artist")),
                "Unknown Artist",
            )
            album = next(
                (source["album"] for source in sources if source.get("album")),
                resolved.get("title") or "Unknown Album",
            )
            year_raw = next(
                (source["year"] for source in sources if source.get("year")),
                None,
            )
            year = str(year_raw) if year_raw else "Unknown"
            return {
                "title": str(title),
                "artist": str(artist),
                "album": clean_album_name(str(album)),
                "year": year[:4] if year != "Unknown" else year,
            }

    info_path = sidecar_path(path, ".info.json")

    if info_path.exists():
        try:
            data = json.loads(info_path.read_text())
            title: str = data.get("title") or path.stem
            artist: str = data.get("artist") or ""
            uploader: str = data.get("uploader") or "Unknown Artist"
            album: str = data.get("album") or title
            year_raw: str = data.get("upload_date") or data.get("date") or ""
            year = year_raw[:4] if year_raw else "Unknown"

            if not artist or artist == "null":
                # Parse "Artist - Album (Year)" from the YouTube title
                parsed_artist, parsed_album, parsed_year = _parse_title(title)
                artist = parsed_artist or uploader
                # Only use parsed album if no explicit album tag
                if not data.get("album"):
                    album = parsed_album or title
                if parsed_year:
                    year = parsed_year

            return {
                "title": title,
                "artist": artist,
                "album": clean_album_name(album),
                "year": year,
            }
        except Exception:
            pass

    # Fallback: embedded tags
    try:
        data = probe(path)
        tags: dict = {}
        for stream in data.get("streams", []):
            if stream.get("codec_type") == "audio" or stream.get("tags"):
                tags.update(stream.get("tags", {}))
                break
        tags.update(data.get("format", {}).get("tags", {}))
        # Tag keys vary by format; normalize them to lower case.
        tags = {k.lower(): v for k, v in tags.items()}
        date_raw = tags.get("date", "")
        return {
            "title": tags.get("title", path.stem),
            "artist": tags.get("artist", "Unknown Artist"),
            "album": clean_album_name(tags.get("album", "Unknown Album")),
            "year": date_raw[:4] if date_raw else "Unknown",
        }
    except Exception:
        pass

    return {
        "title": path.stem,
        "artist": "Unknown Artist",
        "album": "Unknown Album",
        "year": "Unknown",
    }

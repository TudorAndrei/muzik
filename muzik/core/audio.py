"""ffprobe wrappers and audio metadata helpers."""

import json
import re
from pathlib import Path
from typing import Optional

from muzik.core.runner import run_silent


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
    return artist, album, year


def probe(path: Path) -> dict:
    """Run ffprobe on *path* and return parsed JSON.

    Raises ValueError if ffprobe fails.
    """
    result = run_silent(
        [
            "ffprobe",
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            "-show_chapters",
            str(path),
        ]
    )
    if result.returncode != 0:
        raise ValueError(f"ffprobe failed for {path}: {result.stderr.strip()}")
    return json.loads(result.stdout)


def get_duration(path: Path) -> Optional[float]:
    """Return audio duration in seconds, or None on failure."""
    try:
        data = probe(path)
        return float(data["format"]["duration"])
    except (KeyError, ValueError, TypeError):
        return None


def extract_metadata(path: Path) -> dict:
    """Return a dict with keys: title, artist, album, year.

    Preference order:
    1. Sidecar .info.json (yt-dlp metadata)
    2. ffprobe embedded tags
    3. Reasonable fallbacks
    """
    base = path.with_suffix("")
    info_path = base.with_suffix(".info.json")

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
                if parsed_year and year == "Unknown":
                    year = parsed_year

            return {
                "title": title,
                "artist": artist,
                "album": album,
                "year": year,
            }
        except Exception:
            pass

    # Fallback: ffprobe embedded tags
    try:
        data = probe(path)
        tags: dict = data.get("format", {}).get("tags", {})
        # ffprobe tags are case-insensitive in practice; normalise to lower
        tags = {k.lower(): v for k, v in tags.items()}
        date_raw = tags.get("date", "")
        return {
            "title": tags.get("title", path.stem),
            "artist": tags.get("artist", "Unknown Artist"),
            "album": tags.get("album", "Unknown Album"),
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

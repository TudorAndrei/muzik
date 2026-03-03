"""MusicBrainz lookup helpers (uses musicbrainzngs).

Used as a fallback when an audio file has no chapter markers but appears to be
an album (long duration, artist/album identifiable from filename or info.json).
"""

from typing import Optional

import musicbrainzngs

from muzik.core.chapters import Chapter

musicbrainzngs.set_useragent(
    "music-tools", "0.1.0", "https://github.com/local/music-tools"
)

# Minimum file duration (seconds) below which we don't try MusicBrainz
MIN_ALBUM_DURATION = 8 * 60  # 8 minutes


# ---------------------------------------------------------------------------
# Public API
# ---------------------------------------------------------------------------


def search_releases(
    artist: str,
    album: str,
    year: Optional[str] = None,
    limit: int = 5,
) -> list[dict]:
    """Search MusicBrainz for releases matching *artist* and *album*.

    Returns a list of release dicts sorted by score (best first).
    """
    query_parts = [f'release:"{album}"']
    if artist:
        query_parts.append(f'artist:"{artist}"')
    if year and year != "Unknown":
        query_parts.append(f"date:{year}")

    result = musicbrainzngs.search_releases(
        query=" AND ".join(query_parts),
        limit=limit,
    )
    return result.get("release-list", [])


def get_tracklist(release_id: str) -> list[dict]:
    """Fetch full track listing for a MusicBrainz *release_id*.

    Each returned dict has: ``title`` (str), ``position`` (int),
    ``length`` (int | None, milliseconds).
    """
    result = musicbrainzngs.get_release_by_id(release_id, includes=["recordings"])
    release = result.get("release", {})
    tracks: list[dict] = []
    for medium in release.get("medium-list", []):
        for t in medium.get("track-list", []):
            recording = t.get("recording", {})
            length = t.get("length") or recording.get("length")
            tracks.append(
                {
                    "title": t.get("title")
                    or recording.get("title", f"Track {t.get('position', '?')}"),
                    "position": int(t.get("position", 0)),
                    "length": int(length) if length else None,
                }
            )
    return tracks


def tracks_to_chapters(tracks: list[dict]) -> list[Chapter]:
    """Convert MusicBrainz tracks to :class:`Chapter` objects.

    Uses cumulative track lengths to derive start/end timestamps.
    Returns an empty list if any track is missing its length.
    """
    pos = 0  # seconds
    chapters: list[Chapter] = []
    for i, t in enumerate(tracks):
        ms = t.get("length")
        if ms is None:
            return []
        length_s = ms // 1000
        end = pos + length_s
        chapters.append(Chapter(index=i + 1, start=pos, end=end, title=t["title"]))
        pos = end
    return chapters


def lookup_chapters(
    artist: str,
    album: str,
    year: Optional[str] = None,
) -> tuple[list[Chapter], str]:
    """High-level helper: search → best match → chapters + release title.

    Tries multiple query variants to maximise match rate:
    1. artist + album (no year)
    2. artist with common suffixes stripped + album
    3. album only

    Returns ``([], "")`` on any failure (network, not found, missing lengths).
    """
    # Build a list of (artist, year) variants to try
    artist_variants = [artist]
    # Strip common YouTube channel suffixes
    for suffix in (" Project", " Band", " Trio", " Quartet", " Orchestra", " Ensemble"):
        if artist.lower().endswith(suffix.lower()):
            artist_variants.append(artist[: -len(suffix)].strip())

    # Clean up album name — strip parenthetical years and common YouTube suffixes
    import re as _re

    clean_album = _re.sub(r"\s*[\(\[](?:19|20)\d{2}[\)\]]", "", album).strip()
    clean_album = _re.sub(
        r"\s*[\(\[]\s*(?:full\s+album|complete\s+album|full\s+lp|official\s+album"
        r"|remaster(?:ed)?|deluxe(?:\s+edition)?|bonus\s+tracks?)\s*[\)\]]",
        "",
        clean_album,
        flags=_re.IGNORECASE,
    ).strip()

    # Don't filter by year if it looks like an upload year (user uploaded 2019, album from 1998)
    # Just skip year to broaden the search
    queries: list[tuple[str, Optional[str]]] = []
    for av in artist_variants:
        queries.append((av, None))
    queries.append(("", None))  # album-only fallback

    try:
        for q_artist, q_year in queries:
            releases = search_releases(q_artist, clean_album, q_year, limit=5)
            if releases:
                best = releases[0]
                score = int(best.get("score", 0))
                if score < 80:
                    continue
                rid = best.get("id", "")
                release_title = best.get("title", album)
                if not rid:
                    continue
                tracks = get_tracklist(rid)
                chapters = tracks_to_chapters(tracks)
                if chapters:
                    return chapters, release_title
        return [], ""
    except Exception as exc:
        return [], f"error: {exc}"


def lookup_chapters_verbose(
    artist: str,
    album: str,
    year: Optional[str] = None,
) -> tuple[list["Chapter"], str, str]:
    """Like ``lookup_chapters`` but also returns a diagnostic message."""
    import re as _re

    artist_variants = [artist]
    for suffix in (" Project", " Band", " Trio", " Quartet", " Orchestra", " Ensemble"):
        if artist.lower().endswith(suffix.lower()):
            artist_variants.append(artist[: -len(suffix)].strip())

    clean_album = _re.sub(r"\s*[\(\[](?:19|20)\d{2}[\)\]]", "", album).strip()
    clean_album = _re.sub(
        r"\s*[\(\[]\s*(?:full\s+album|complete\s+album|full\s+lp|official\s+album"
        r"|remaster(?:ed)?|deluxe(?:\s+edition)?|bonus\s+tracks?)\s*[\)\]]",
        "",
        clean_album,
        flags=_re.IGNORECASE,
    ).strip()

    queries: list[tuple[str, Optional[str]]] = []
    for av in artist_variants:
        queries.append((av, None))
    queries.append(("", None))

    diag_lines: list[str] = [f"cleaned album: {clean_album!r}"]

    try:
        for q_artist, q_year in queries:
            releases = search_releases(q_artist, clean_album, q_year, limit=5)
            if not releases:
                diag_lines.append(
                    f"  query ({q_artist!r}, {clean_album!r}): no results"
                )
                continue
            scores = [(r.get("title", "?"), int(r.get("score", 0))) for r in releases]
            diag_lines.append(
                f"  query ({q_artist!r}, {clean_album!r}): "
                + ", ".join(f"{t!r}={s}" for t, s in scores)
            )
            best = releases[0]
            score = int(best.get("score", 0))
            if score < 80:
                continue
            rid = best.get("id", "")
            release_title = best.get("title", album)
            if not rid:
                continue
            tracks = get_tracklist(rid)
            chapters = tracks_to_chapters(tracks)
            if not chapters:
                diag_lines.append(
                    f"  {release_title!r}: tracks found but lengths missing"
                )
                continue
            return chapters, release_title, "\n".join(diag_lines)
        return [], "", "\n".join(diag_lines)
    except Exception as exc:
        return [], "", "\n".join(diag_lines) + f"\n  exception: {exc}"

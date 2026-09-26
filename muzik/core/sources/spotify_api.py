"""Read-only Spotify Web API client.

muzik never downloads Spotify media. This client reads playlist and track
metadata only. The audio comes from Soulseek, as with a Spotify export file.

Every response becomes a canonical Spotify export document, which
``muzik.core.sources.spotify.parse_playlist_json`` then validates. The API
path and the file path therefore produce the same values.
"""

from __future__ import annotations

from collections.abc import Callable, Iterator, Mapping
from dataclasses import dataclass
import json
import time
from typing import Any, cast
import urllib.error
import urllib.parse
import urllib.request

from muzik.core.sources.base import ResolvedPlaylist
from muzik.core.sources.spotify import parse_playlist_json
from muzik.core.sources.spotify_auth import (
    SpotifyAuthError,
    SpotifyTokens,
    TokenStore,
    current_tokens,
    refresh_tokens,
)


API_ROOT = "https://api.spotify.com/v1"
LIKED_URI = "spotify:liked"
LIKED_NAME = "Liked Songs"
PAGE_SIZE = 50
_MAX_RETRIES = 3

Requester = Callable[[str, Mapping[str, str]], dict[str, Any]]


class SpotifyApiError(RuntimeError):
    """Raised when the Spotify API cannot answer a request."""


class SpotifyNotFoundError(SpotifyApiError):
    """Raised when an endpoint or an object does not exist."""


@dataclass(frozen=True, slots=True)
class SpotifyPlaylistRef:
    """One playlist, album, or saved-track collection of the user."""

    uri: str
    name: str
    owner: str = ""
    total: int | None = None
    image_url: str | None = None

    @property
    def url(self) -> str:
        if self.uri == LIKED_URI:
            return "https://open.spotify.com/collection/tracks"
        kind, identifier = _split_uri(self.uri)
        return f"https://open.spotify.com/{kind}/{identifier}"


def get_json(url: str, headers: Mapping[str, str]) -> dict[str, Any]:
    """Send one GET request and return the JSON body."""
    request = urllib.request.Request(url, headers=dict(headers), method="GET")
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            payload = json.loads(response.read().decode("utf-8"))
    except urllib.error.HTTPError as exc:
        raise http_error(exc) from exc
    except (urllib.error.URLError, TimeoutError) as exc:
        raise SpotifyApiError(f"Unable to reach Spotify: {exc}") from exc
    except json.JSONDecodeError as exc:
        raise SpotifyApiError("Spotify returned an invalid response.") from exc
    if not isinstance(payload, dict):
        raise SpotifyApiError("Spotify returned an invalid response.")
    return cast(dict[str, Any], payload)


class _HttpStatusError(SpotifyApiError):
    """Carry the status code of a failed request."""

    def __init__(self, status: int, message: str, retry_after: float = 0.0) -> None:
        super().__init__(message)
        self.status = status
        self.retry_after = retry_after


def http_error(error: urllib.error.HTTPError) -> SpotifyApiError:
    """Convert one failed request into an error that keeps its status code."""
    detail = ""
    try:
        body = json.loads(error.read().decode("utf-8"))
        if isinstance(body, dict):
            inner = body.get("error")
            if isinstance(inner, dict):
                detail = str(inner.get("message") or "").strip()
            elif isinstance(inner, str):
                detail = inner.strip()
    except OSError, ValueError:
        detail = ""
    retry_after = 0.0
    header = error.headers.get("Retry-After") if error.headers else None
    if header:
        try:
            retry_after = float(header)
        except ValueError:
            retry_after = 0.0
    if error.code == 403:
        detail = (
            f"{detail or 'Forbidden'}. A Development Mode application only "
            "answers for users that you added to it, and its owner needs a "
            "Spotify Premium account."
        )
    return _HttpStatusError(
        error.code, f"Spotify: {detail or error.reason}", retry_after
    )


class SpotifyClient:
    """Read playlists and tracks for the connected Spotify user."""

    def __init__(
        self,
        *,
        tokens: SpotifyTokens | None = None,
        settings: Mapping[str, str] | None = None,
        store: TokenStore | None = None,
        requester: Requester = get_json,
        sleeper: Callable[[float], None] = time.sleep,
    ) -> None:
        self._tokens = tokens
        self._settings = dict(settings) if settings is not None else None
        self._store = store
        self._requester = requester
        self._sleeper = sleeper

    def _access_token(self) -> str:
        if self._tokens is None or self._tokens.is_expired():
            self._tokens = current_tokens(settings=self._settings, store=self._store)
        return self._tokens.access_token

    def _get(self, url: str) -> dict[str, Any]:
        if not url.startswith("http"):
            url = f"{API_ROOT}{url}"
        refreshed = False
        for attempt in range(_MAX_RETRIES):
            headers = {"Authorization": f"Bearer {self._access_token()}"}
            try:
                return self._requester(url, headers)
            except _HttpStatusError as exc:
                if exc.status == 401 and not refreshed and self._tokens is not None:
                    self._tokens = refresh_tokens(
                        self._tokens, settings=self._settings, store=self._store
                    )
                    refreshed = True
                    continue
                if exc.status == 429 and attempt + 1 < _MAX_RETRIES:
                    self._sleeper(min(exc.retry_after or 1.0, 30.0))
                    continue
                if exc.status == 404:
                    raise SpotifyNotFoundError(str(exc)) from exc
                raise SpotifyApiError(str(exc)) from exc
        raise SpotifyApiError("Spotify did not answer after several tries.")

    def _get_first_available(self, urls: tuple[str, ...]) -> dict[str, Any]:
        """Return the first endpoint that exists.

        Spotify renamed several endpoints in 2026. muzik asks for the current
        name first and keeps the older name as a fallback.
        """
        last: SpotifyApiError | None = None
        for url in urls:
            try:
                return self._get(url)
            except SpotifyNotFoundError as exc:
                last = exc
        raise last or SpotifyApiError("No Spotify endpoint answered.")

    def _pages(self, urls: tuple[str, ...]) -> Iterator[dict[str, Any]]:
        page = self._get_first_available(urls)
        while True:
            yield page
            next_url = page.get("next")
            if not isinstance(next_url, str) or not next_url:
                return
            page = self._get(next_url)

    def _items(self, urls: tuple[str, ...]) -> Iterator[Any]:
        for page in self._pages(urls):
            for item in page.get("items") or []:
                yield item

    def account_name(self) -> str:
        """Return the display name of the connected account."""
        payload = self._get("/me")
        name = payload.get("display_name") or payload.get("id") or ""
        return str(name)

    def list_playlists(self) -> list[SpotifyPlaylistRef]:
        """Return Liked Songs and every playlist of the user."""
        references = [SpotifyPlaylistRef(uri=LIKED_URI, name=LIKED_NAME, owner="you")]
        for raw in self._items((f"/me/playlists?limit={PAGE_SIZE}",)):
            if not isinstance(raw, dict):
                continue
            identifier = raw.get("id")
            if not isinstance(identifier, str) or not identifier:
                continue
            owner = raw.get("owner")
            references.append(
                SpotifyPlaylistRef(
                    uri=f"spotify:playlist:{identifier}",
                    name=str(raw.get("name") or identifier),
                    owner=str(
                        (owner or {}).get("display_name")
                        or (owner or {}).get("id")
                        or ""
                    ),
                    total=_total_of(raw),
                    image_url=_first_image(raw.get("images")),
                )
            )
        return references

    def load_playlist(self, uri: str) -> ResolvedPlaylist:
        """Return the current tracks of one playlist, album, or Liked Songs."""
        kind, identifier = _split_uri(uri)
        if kind == "liked":
            return self._load_liked()
        if kind == "playlist":
            return self._load_playlist(identifier)
        if kind == "album":
            return self._load_album(identifier)
        raise SpotifyApiError(f"muzik cannot read the Spotify reference {uri!r}.")

    def _load_liked(self) -> ResolvedPlaylist:
        entries = _entries(
            self._items(
                (
                    f"/me/tracks?limit={PAGE_SIZE}",
                    f"/me/library?type=track&limit={PAGE_SIZE}",
                )
            )
        )
        return parse_playlist_json(
            _export_document(playlist_id="liked", title=LIKED_NAME, entries=entries)
        )

    def _load_playlist(self, identifier: str) -> ResolvedPlaylist:
        details = self._get(f"/playlists/{identifier}")
        entries = _entries(
            self._items(
                (
                    f"/playlists/{identifier}/items?limit={PAGE_SIZE}",
                    f"/playlists/{identifier}/tracks?limit={PAGE_SIZE}",
                )
            )
        )
        return parse_playlist_json(
            _export_document(
                playlist_id=identifier,
                title=str(details.get("name") or identifier),
                entries=entries,
                snapshot_id=details.get("snapshot_id"),
            )
        )

    def _load_album(self, identifier: str) -> ResolvedPlaylist:
        album = self._get(f"/albums/{identifier}")
        image = _first_image(album.get("images"))
        entries: list[dict[str, Any]] = []
        for index, raw in enumerate(
            self._items((f"/albums/{identifier}/tracks?limit={PAGE_SIZE}",)), start=1
        ):
            if not isinstance(raw, dict):
                continue
            entry = _entry_from_track(raw, index=index, album=album, image_url=image)
            if entry is not None:
                entries.append(entry)
        return parse_playlist_json(
            _export_document(
                playlist_id=identifier,
                title=str(album.get("name") or identifier),
                entries=_renumbered(entries),
            )
        )


def playlist_reference(uri: str, *, name: str = "") -> SpotifyPlaylistRef:
    """Return a reference for *uri* without calling Spotify."""
    kind, identifier = _split_uri(uri)
    if kind == "liked":
        return SpotifyPlaylistRef(uri=LIKED_URI, name=name or LIKED_NAME)
    return SpotifyPlaylistRef(uri=uri, name=name or identifier)


def is_readable(uri: str) -> bool:
    """Return whether the API client can read this reference."""
    kind, identifier = _split_uri(uri)
    return kind in {"liked", "playlist", "album"} and (
        kind == "liked" or bool(identifier)
    )


def _split_uri(uri: str) -> tuple[str, str]:
    parts = uri.split(":")
    if len(parts) == 2 and parts[0] == "spotify":
        return parts[1], ""
    if len(parts) == 3 and parts[0] == "spotify":
        return parts[1], parts[2]
    return "", ""


def _total_of(raw: Mapping[str, Any]) -> int | None:
    for key in ("items", "tracks"):
        value = raw.get(key)
        if isinstance(value, dict) and isinstance(value.get("total"), int):
            return int(value["total"])
    return None


def _first_image(images: Any) -> str | None:
    if not isinstance(images, list):
        return None
    for image in images:
        if isinstance(image, dict) and isinstance(image.get("url"), str):
            return image["url"]
    return None


def _entries(items: Iterator[Any]) -> list[dict[str, Any]]:
    entries: list[dict[str, Any]] = []
    for index, raw in enumerate(items, start=1):
        if not isinstance(raw, dict):
            continue
        # The 2026 rename made "track" into "item"; both shapes are accepted.
        track = raw.get("item") or raw.get("track")
        if not isinstance(track, dict):
            continue
        entry = _entry_from_track(track, index=index, added_at=raw.get("added_at"))
        if entry is not None:
            entries.append(entry)
    return _renumbered(entries)


def _renumbered(entries: list[dict[str, Any]]) -> list[dict[str, Any]]:
    for position, entry in enumerate(entries, start=1):
        entry["index"] = position
    return entries


def _entry_from_track(
    track: Mapping[str, Any],
    *,
    index: int,
    album: Mapping[str, Any] | None = None,
    image_url: str | None = None,
    added_at: Any = None,
) -> dict[str, Any] | None:
    """Return one canonical export entry, or None for a skipped item.

    Podcast episodes and local files are skipped: muzik cannot acquire them
    from their metadata.
    """
    if track.get("type") == "episode" or track.get("is_local"):
        return None
    title = str(track.get("name") or "").strip()
    if not title:
        return None
    artists = [
        str(artist.get("name") or "").strip()
        for artist in track.get("artists") or []
        if isinstance(artist, dict) and str(artist.get("name") or "").strip()
    ]
    album_data = album if album is not None else track.get("album")
    if not isinstance(album_data, Mapping):
        album_data = {}
    if not artists:
        artists = [
            str(artist.get("name") or "").strip()
            for artist in album_data.get("artists") or []
            if isinstance(artist, dict) and str(artist.get("name") or "").strip()
        ]
    if not artists:
        return None
    uri = track.get("uri")
    identifier = track.get("id")
    source_id = (
        str(uri)
        if isinstance(uri, str) and uri.startswith("spotify:track:")
        else (f"spotify:track:{identifier}" if isinstance(identifier, str) else None)
    )
    external = track.get("external_ids")
    isrc = (
        str(external.get("isrc"))
        if isinstance(external, Mapping) and external.get("isrc")
        else None
    )
    image = image_url or _first_image(album_data.get("images"))
    entry: dict[str, Any] = {
        "index": index,
        "title": title,
        "artists": artists,
        "source_id": source_id,
        "source_url": _track_url(track),
    }
    if album_data.get("name"):
        entry["album"] = str(album_data["name"])
    if album_data.get("release_date"):
        entry["release_date"] = str(album_data["release_date"])
    if isinstance(track.get("duration_ms"), (int, float)):
        entry["duration_ms"] = track["duration_ms"]
    if isinstance(track.get("disc_number"), int):
        entry["disc_number"] = track["disc_number"]
    if isinstance(track.get("track_number"), int):
        entry["track_number"] = track["track_number"]
    if isrc:
        entry["isrc"] = isrc
    if isinstance(added_at, str) and added_at:
        entry["added_at"] = added_at
    if image:
        entry["source_metadata"] = {"image": image}
    return entry


def _track_url(track: Mapping[str, Any]) -> str | None:
    external = track.get("external_urls")
    if isinstance(external, Mapping) and isinstance(external.get("spotify"), str):
        return external["spotify"]
    identifier = track.get("id")
    if isinstance(identifier, str) and identifier:
        return f"https://open.spotify.com/track/{identifier}"
    return None


def _export_document(
    *,
    playlist_id: str,
    title: str,
    entries: list[dict[str, Any]],
    snapshot_id: Any = None,
) -> dict[str, Any]:
    document: dict[str, Any] = {
        "version": 1,
        "source": "spotify",
        "type": "playlist",
        "id": playlist_id,
        "title": title,
        "entries": entries,
    }
    if isinstance(snapshot_id, str) and snapshot_id:
        document["snapshot_id"] = snapshot_id
    return document


def export_document(playlist: ResolvedPlaylist) -> dict[str, Any]:
    """Return a canonical export document for a playlist read from the API."""
    entries: list[dict[str, Any]] = []
    for track in playlist.entries:
        metadata = dict(getattr(track, "source_metadata", {}) or {})
        entry: dict[str, Any] = {
            "index": getattr(track, "index", None) or len(entries) + 1,
            "title": track.title,
            "artists": metadata.get("artists") or [getattr(track, "artist", "")],
            "source_id": getattr(track, "source_id", None),
            "source_url": getattr(track, "source_url", None),
        }
        for key, value in (
            ("album", getattr(track, "album", None)),
            ("release_date", getattr(track, "year", None)),
            ("isrc", metadata.get("isrc")),
            ("disc_number", metadata.get("disc_number")),
            ("track_number", metadata.get("track_number")),
            ("added_at", metadata.get("added_at")),
        ):
            if value:
                entry[key] = value
        duration = getattr(track, "duration", None)
        if duration:
            entry["duration_ms"] = int(float(duration) * 1000)
        if metadata.get("image"):
            entry["source_metadata"] = {"image": metadata["image"]}
        entries.append(entry)
    return _export_document(
        playlist_id=playlist.source_id or "playlist",
        title=playlist.title,
        entries=entries,
        snapshot_id=playlist.source_metadata.get("snapshot_id"),
    )


__all__ = [
    "LIKED_NAME",
    "LIKED_URI",
    "SpotifyApiError",
    "SpotifyAuthError",
    "SpotifyClient",
    "SpotifyNotFoundError",
    "SpotifyPlaylistRef",
    "export_document",
    "get_json",
    "is_readable",
    "playlist_reference",
]

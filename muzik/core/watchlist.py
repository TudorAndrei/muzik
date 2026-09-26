"""Durable watchlist records for saved external playlist sources."""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass, field
from datetime import datetime
from enum import Enum
import json
import os
from pathlib import Path
import re
import tempfile
from typing import Any, cast

from beets.library import Library

from muzik.config import MUZIK_WATCHLIST_FILE
from muzik.core.beets.config import open_library
from muzik.core.beets.lookup import find_organized_path, find_path_by_source_id
from muzik.core.quality import QualityPolicy
from muzik.core.sources.base import ResolvedPlaylist, ResolvedTrack
from muzik.core.sources.spotify import parse_link as parse_spotify_link
from muzik.core.sources.spotify_api import (
    LIKED_NAME,
    LIKED_URI,
    SpotifyApiError,
    SpotifyClient,
    is_readable as is_readable_spotify_reference,
)
from muzik.core.sources.spotify_auth import SpotifyAuthError
from muzik.core.sources.youtube import (
    PlaylistLookupError,
    YouTubePlaylist,
    YouTubePlaylistItem,
    find_audio_by_id,
    get_playlist,
    playlist_id,
)
from muzik.core.workflow.cancellation import CancellationToken, WorkflowCancelled
from muzik.core.workflow.events import (
    MessageEvent,
    NullWorkflowEventEmitter,
    ProgressAdvancedEvent,
    ProgressFinishedEvent,
    ProgressStartedEvent,
    WorkflowEventEmitter,
)
from muzik.core.workflow.service import (
    PlaylistTrackResult,
    PlaylistVideoResult,
    WorkflowOptions,
    WorkflowRequest,
    WorkflowRunOperations,
    backfill_playlist_entry_from_legacy_cache,
    find_audio_inputs,
    load_playlist_state,
    resolved_track_entries,
    run_resolved_playlist_tracks,
    run_youtube_playlist_videos,
)


WATCHLIST_VERSION = 3
# Version 1 records have no "quality" key in their saved stages dict, so
# _item_from_data's data-driven loop below leaves it at its NOT_STARTED
# default — no separate migration step is needed beyond accepting the old
# version number and always saving the current one back. Version 2 records
# have no "kind" or "title" key; _playlist_from_data defaults them to a
# YouTube source with no name.
SUPPORTED_WATCHLIST_VERSIONS = (1, 2, WATCHLIST_VERSION)
STAGE_NAMES = ("download", "quality", "parse", "split", "organize")


SpotifyLoader = Callable[[str], ResolvedPlaylist]


class WatchlistError(RuntimeError):
    """Base error for watchlist operations."""


class WatchlistFormatError(WatchlistError):
    """Raised when a saved watchlist has invalid data."""


class InvalidPlaylistUrlError(WatchlistError):
    """Raised when a URL contains no known playlist reference."""


class DuplicatePlaylistError(WatchlistError):
    """Raised when the watchlist already contains a playlist ID."""


class WatchlistSourceKind(str, Enum):
    """The external service that a saved watchlist entry points to."""

    YOUTUBE = "youtube"
    SPOTIFY = "spotify"


@dataclass(frozen=True, slots=True)
class WatchlistSource:
    """One external playlist reference, taken from a link."""

    kind: WatchlistSourceKind
    source_id: str
    url: str
    title: str | None = None


_LIKED_ALIASES = (
    LIKED_URI,
    "liked",
    "liked songs",
    "https://open.spotify.com/collection/tracks",
)


def parse_source(value: str) -> WatchlistSource:
    """Return the external playlist reference in *value*.

    A YouTube playlist, a Spotify playlist or album, and the Spotify Liked
    Songs collection are all accepted. A Spotify source needs the Web API.
    """
    text = value.strip()
    if text.lower().rstrip("/") in _LIKED_ALIASES:
        return WatchlistSource(
            kind=WatchlistSourceKind.SPOTIFY,
            source_id=LIKED_URI,
            url="https://open.spotify.com/collection/tracks",
            title=LIKED_NAME,
        )
    youtube_id = playlist_id(text)
    if youtube_id:
        return WatchlistSource(
            kind=WatchlistSourceKind.YOUTUBE,
            source_id=youtube_id,
            url=f"https://www.youtube.com/playlist?list={youtube_id}",
        )
    spotify = parse_spotify_link(text)
    if spotify is not None:
        return WatchlistSource(
            kind=WatchlistSourceKind.SPOTIFY,
            source_id=spotify.uri,
            url=spotify.url,
        )
    raise InvalidPlaylistUrlError(
        "Enter a YouTube playlist URL, a Spotify playlist link, or 'liked'."
    )


class StageStatus(str, Enum):
    NOT_STARTED = "not_started"
    RUNNING = "running"
    COMPLETE = "complete"
    FAILED = "failed"
    SKIPPED = "skipped"
    STALE = "stale"


@dataclass(slots=True)
class StageRecord:
    status: StageStatus = StageStatus.NOT_STARTED
    updated_at: str | None = None
    path: str | None = None
    error: str | None = None

    def to_dict(self) -> dict[str, str | None]:
        return {
            "status": self.status.value,
            "updated_at": self.updated_at,
            "path": self.path,
            "error": self.error,
        }


def new_stage_records() -> dict[str, StageRecord]:
    return {name: StageRecord() for name in STAGE_NAMES}


@dataclass(slots=True)
class WatchlistItem:
    position: int
    title: str
    video_id: str | None
    video_url: str | None = None
    thumbnail_url: str | None = None
    kind: str = WatchlistSourceKind.YOUTUBE.value
    # Spotify only: the playlist-state key of this track, and the resolved
    # track itself, so that one card can run the track again on its own.
    entry_id: str | None = None
    track: dict[str, Any] | None = None
    stages: dict[str, StageRecord] = field(default_factory=new_stage_records)
    last_action: str | None = None
    last_error: str | None = None

    @property
    def key(self) -> str:
        return self.entry_id or self.video_id or f"unavailable:{self.position}"

    @property
    def source_kind(self) -> WatchlistSourceKind:
        return WatchlistSourceKind(self.kind)

    @property
    def resolved_track(self) -> ResolvedTrack | None:
        """Return the saved Spotify track, if this item has one."""
        if not self.track:
            return None
        fields = {name for name in ResolvedTrack.__dataclass_fields__}
        return ResolvedTrack(
            **{key: value for key, value in self.track.items() if key in fields}
        )

    @classmethod
    def from_youtube(cls, item: YouTubePlaylistItem) -> WatchlistItem:
        return cls(
            position=item.position,
            title=item.title,
            video_id=item.video_id,
            video_url=item.video_url,
            thumbnail_url=item.thumbnail_url,
        )

    @classmethod
    def from_track(
        cls,
        track: ResolvedTrack,
        entry_id: str,
        *,
        position: int,
    ) -> WatchlistItem:
        """Build one card from a Spotify track."""
        identifier = (track.source_id or "").rpartition(":")[2] or None
        title = f"{track.artist} - {track.title}" if track.artist else track.title
        image = track.source_metadata.get("image")
        return cls(
            position=track.index or position,
            title=title,
            video_id=identifier,
            video_url=track.source_url,
            thumbnail_url=image if isinstance(image, str) else None,
            kind=WatchlistSourceKind.SPOTIFY.value,
            entry_id=entry_id,
            track=track.to_dict(),
        )

    def to_dict(self) -> dict[str, Any]:
        return {
            "position": self.position,
            "title": self.title,
            "video_id": self.video_id,
            "video_url": self.video_url,
            "thumbnail_url": self.thumbnail_url,
            "kind": self.kind,
            "entry_id": self.entry_id,
            "track": self.track,
            "stages": {name: record.to_dict() for name, record in self.stages.items()},
            "last_action": self.last_action,
            "last_error": self.last_error,
        }


@dataclass(slots=True)
class WatchlistPlaylist:
    playlist_id: str
    url: str
    kind: str = WatchlistSourceKind.YOUTUBE.value
    title: str | None = None
    items: list[WatchlistItem] = field(default_factory=list)
    processed_video_ids: list[str] = field(default_factory=list)
    last_checked_at: str | None = None
    last_error: str | None = None

    @property
    def source_kind(self) -> WatchlistSourceKind:
        return WatchlistSourceKind(self.kind)

    @property
    def display_name(self) -> str:
        return self.title or self.playlist_id

    @property
    def is_refreshable(self) -> bool:
        """Return whether muzik can read new items from this source.

        A Spotify source needs the Web API, thus it also needs a client ID
        and a connected account. Those are checked at refresh time.
        """
        if self.source_kind is WatchlistSourceKind.YOUTUBE:
            return True
        return is_readable_spotify_reference(self.playlist_id)

    @property
    def state_id(self) -> str:
        """Return the playlist-state key that holds the results of this source."""
        if self.source_kind is WatchlistSourceKind.YOUTUBE:
            return self.playlist_id
        identifier = self.playlist_id.rpartition(":")[2] or self.playlist_id
        return f"spotify_{re.sub(r'[^A-Za-z0-9_-]', '_', identifier)}"

    def to_dict(self) -> dict[str, Any]:
        return {
            "playlist_id": self.playlist_id,
            "url": self.url,
            "kind": self.kind,
            "title": self.title,
            "items": [item.to_dict() for item in self.items],
            "processed_video_ids": list(self.processed_video_ids),
            "last_checked_at": self.last_checked_at,
            "last_error": self.last_error,
        }


@dataclass(slots=True)
class Watchlist:
    playlists: list[WatchlistPlaylist] = field(default_factory=list)
    version: int = WATCHLIST_VERSION

    def to_dict(self) -> dict[str, Any]:
        return {
            "version": self.version,
            "playlists": [playlist.to_dict() for playlist in self.playlists],
        }


@dataclass(frozen=True, slots=True)
class WatchlistRefreshSummary:
    playlists_checked: int = 0
    pending_videos: int = 0
    completed_videos: int = 0
    failed_videos: int = 0
    playlist_errors: int = 0


class WatchlistRepository:
    """Load and save one versioned watchlist JSON document."""

    def __init__(self, path: Path | None = None) -> None:
        self.path = path if path is not None else MUZIK_WATCHLIST_FILE

    def load(self) -> Watchlist:
        if not self.path.exists():
            return Watchlist()
        try:
            raw = json.loads(self.path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            raise WatchlistFormatError(
                f"Unable to read watchlist file {self.path}: {exc}"
            ) from exc
        return _watchlist_from_data(raw)

    def save(self, watchlist: Watchlist) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        temp_path: Path | None = None
        try:
            with tempfile.NamedTemporaryFile(
                mode="w",
                encoding="utf-8",
                dir=self.path.parent,
                prefix=f".{self.path.name}.",
                suffix=".tmp",
                delete=False,
            ) as handle:
                temp_path = Path(handle.name)
                json.dump(watchlist.to_dict(), handle, indent=2, ensure_ascii=False)
                handle.write("\n")
                handle.flush()
                os.fsync(handle.fileno())
            temp_path.replace(self.path)
        except OSError as exc:
            raise WatchlistError(
                f"Unable to save watchlist file {self.path}: {exc}"
            ) from exc
        finally:
            if temp_path is not None and temp_path.exists():
                temp_path.unlink()

    def add(self, url: str) -> WatchlistPlaylist:
        source = parse_source(url)
        watchlist = self.load()
        if any(item.playlist_id == source.source_id for item in watchlist.playlists):
            raise DuplicatePlaylistError(
                f"Playlist {source.source_id} is already in the watchlist."
            )
        playlist = WatchlistPlaylist(
            playlist_id=source.source_id,
            url=source.url,
            kind=source.kind.value,
            title=source.title,
        )
        watchlist.playlists.append(playlist)
        self.save(watchlist)
        return playlist

    def rename(self, playlist_id_value: str, title: str) -> bool:
        """Give one saved source a name of the user's choice."""
        watchlist = self.load()
        for playlist in watchlist.playlists:
            if playlist.playlist_id != playlist_id_value:
                continue
            playlist.title = title.strip() or None
            self.save(watchlist)
            return True
        return False

    def remove(self, playlist_id_value: str) -> bool:
        watchlist = self.load()
        remaining = [
            item
            for item in watchlist.playlists
            if item.playlist_id != playlist_id_value
        ]
        if len(remaining) == len(watchlist.playlists):
            return False
        watchlist.playlists = remaining
        self.save(watchlist)
        return True


def refresh_watchlist(
    repository: WatchlistRepository,
    request: WorkflowRequest,
    options: WorkflowOptions,
    *,
    operations: WorkflowRunOperations,
    item_loader: Callable[
        [str], YouTubePlaylist | list[YouTubePlaylistItem]
    ] = get_playlist,
    spotify_loader: SpotifyLoader | None = None,
    events: WorkflowEventEmitter | None = None,
    cancellation: CancellationToken | None = None,
) -> WatchlistRefreshSummary:
    """Refresh every saved source and process only its pending items."""
    events = events or NullWorkflowEventEmitter()
    cancellation = cancellation or CancellationToken()
    watchlist = repository.load()
    task_id = "watchlist-refresh"
    events.emit(
        ProgressStartedEvent(
            task_id=task_id,
            description="Checking watchlist playlists.",
            total=len(watchlist.playlists),
        )
    )
    pending_total = 0
    completed_total = 0
    failed_total = 0
    playlist_errors = 0

    for playlist in watchlist.playlists:
        cancellation.raise_if_cancelled()
        if playlist.source_kind is WatchlistSourceKind.SPOTIFY:
            totals = _refresh_spotify_playlist(
                playlist,
                repository=repository,
                watchlist=watchlist,
                options=options,
                operations=operations,
                loader=spotify_loader or _load_spotify_playlist,
                events=events,
                cancellation=cancellation,
            )
            pending_total += totals.pending
            completed_total += totals.completed
            failed_total += totals.failed
            playlist_errors += totals.errors
            events.emit(ProgressAdvancedEvent(task_id=task_id))
            continue
        if not playlist.is_refreshable:
            events.emit(
                MessageEvent(f"muzik cannot read the source {playlist.display_name}.")
            )
            events.emit(ProgressAdvancedEvent(task_id=task_id))
            continue
        events.emit(MessageEvent(f"Checking playlist {playlist.display_name}."))
        try:
            loaded = item_loader(playlist.url)
        except PlaylistLookupError as exc:
            playlist.last_checked_at = _now()
            playlist.last_error = str(exc)
            repository.save(watchlist)
            playlist_errors += 1
            events.emit(MessageEvent(str(exc), severity="error"))
            events.emit(ProgressAdvancedEvent(task_id=task_id))
            continue

        if isinstance(loaded, YouTubePlaylist):
            playlist.title = loaded.title or playlist.title
            discovered = loaded.items
        else:
            discovered = loaded
        playlist.items = _merge_playlist_items(playlist.items, discovered)
        playlist.last_checked_at = _now()
        playlist.last_error = None
        _reconcile_playlist(playlist, request=request, options=options)
        repository.save(watchlist)

        pending = [
            item.video_id
            for item in playlist.items
            if item.video_id and item.video_id not in playlist.processed_video_ids
        ]
        pending = list(dict.fromkeys(pending))
        pending_total += len(pending)
        events.emit(
            MessageEvent(
                f"Playlist {playlist.playlist_id} has {len(pending)} pending video(s)."
            )
        )

        def save_result(result: PlaylistVideoResult) -> None:
            nonlocal completed_total, failed_total
            matches = [
                item for item in playlist.items if item.video_id == result.video_id
            ]
            if result.completed:
                if result.video_id not in playlist.processed_video_ids:
                    playlist.processed_video_ids.append(result.video_id)
                completed_total += 1
                for item in matches:
                    _mark_workflow_completed(item, options=options)
            else:
                failed_total += 1
                for item in matches:
                    item.last_error = "Download failed. Select Retry to try again."
                    item.last_action = "refresh"
                    item.stages["download"] = StageRecord(
                        status=StageStatus.FAILED,
                        updated_at=_now(),
                        error=item.last_error,
                    )
            repository.save(watchlist)

        try:
            run_youtube_playlist_videos(
                WorkflowRequest(
                    raw=playlist.url,
                    output=request.output,
                    splits=request.splits,
                ),
                options,
                playlist_id=playlist.playlist_id,
                video_ids=pending,
                operations=operations,
                events=events,
                cancellation=cancellation,
                on_result=save_result,
            )
        except WorkflowCancelled:
            raise
        except Exception as exc:
            playlist.last_error = str(exc)
            repository.save(watchlist)
            playlist_errors += 1
            events.emit(
                MessageEvent(
                    f"Playlist {playlist.playlist_id} stopped: {exc}",
                    severity="error",
                )
            )
        events.emit(ProgressAdvancedEvent(task_id=task_id))

    events.emit(
        ProgressFinishedEvent(
            task_id=task_id,
            success=failed_total == 0 and playlist_errors == 0,
        )
    )
    events.emit(
        MessageEvent(
            f"Watchlist refresh complete: {completed_total} processed, "
            f"{failed_total} failed, {playlist_errors} playlist error(s)."
        )
    )
    return WatchlistRefreshSummary(
        playlists_checked=sum(
            1 for playlist in watchlist.playlists if playlist.is_refreshable
        ),
        pending_videos=pending_total,
        completed_videos=completed_total,
        failed_videos=failed_total,
        playlist_errors=playlist_errors,
    )


@dataclass(slots=True)
class _SourceTotals:
    """Counts of one source in a refresh run."""

    pending: int = 0
    completed: int = 0
    failed: int = 0
    errors: int = 0


def _load_spotify_playlist(uri: str) -> ResolvedPlaylist:
    """Read one Spotify reference with the connected account."""
    return SpotifyClient().load_playlist(uri)


def _refresh_spotify_playlist(
    playlist: WatchlistPlaylist,
    *,
    repository: WatchlistRepository,
    watchlist: Watchlist,
    options: WorkflowOptions,
    operations: WorkflowRunOperations,
    loader: SpotifyLoader,
    events: WorkflowEventEmitter,
    cancellation: CancellationToken,
) -> _SourceTotals:
    """Sync one Spotify source and acquire the tracks that are not done."""
    totals = _SourceTotals()
    events.emit(MessageEvent(f"Reading Spotify source {playlist.display_name}."))
    try:
        resolved = loader(playlist.playlist_id)
    except (SpotifyApiError, SpotifyAuthError) as exc:
        playlist.last_checked_at = _now()
        playlist.last_error = str(exc)
        repository.save(watchlist)
        totals.errors = 1
        events.emit(MessageEvent(str(exc), severity="error"))
        return totals

    pairs = resolved_track_entries(resolved)
    playlist.title = resolved.title or playlist.title
    playlist.items = _merge_spotify_items(playlist.items, pairs)
    playlist.last_checked_at = _now()
    playlist.last_error = None
    _reconcile_spotify_playlist(playlist, options=options)
    repository.save(watchlist)

    pending = [
        item.entry_id
        for item in playlist.items
        if item.entry_id and item.entry_id not in playlist.processed_video_ids
    ]
    totals.pending = len(pending)
    events.emit(
        MessageEvent(
            f"Spotify source {playlist.display_name} has "
            f"{len(pending)} pending track(s)."
        )
    )

    def save_result(result: PlaylistTrackResult) -> None:
        matches = [item for item in playlist.items if item.entry_id == result.entry_id]
        if result.completed:
            if result.entry_id not in playlist.processed_video_ids:
                playlist.processed_video_ids.append(result.entry_id)
            totals.completed += 1
            for item in matches:
                _mark_track_completed(item, options=options)
        else:
            totals.failed += 1
            for item in matches:
                item.last_action = "refresh"
                item.last_error = result.error or "Soulseek found no audio."
                item.stages["download"] = StageRecord(
                    status=StageStatus.FAILED,
                    updated_at=_now(),
                    error=item.last_error,
                )
        repository.save(watchlist)

    if not pending:
        return totals
    try:
        run_resolved_playlist_tracks(
            resolved,
            options,
            entry_ids=pending,
            operations=operations,
            events=events,
            cancellation=cancellation,
            on_result=save_result,
        )
    except WorkflowCancelled:
        raise
    except Exception as exc:
        playlist.last_error = str(exc)
        repository.save(watchlist)
        totals.errors += 1
        events.emit(
            MessageEvent(
                f"Spotify source {playlist.display_name} stopped: {exc}",
                severity="error",
            )
        )
    return totals


def _merge_spotify_items(
    existing: list[WatchlistItem],
    pairs: list[tuple[ResolvedTrack, str]],
) -> list[WatchlistItem]:
    """Rebuild the cards of a Spotify source, keeping the stage state."""
    saved = {item.entry_id: item for item in existing if item.entry_id}
    merged: list[WatchlistItem] = []
    for position, (track, entry_id) in enumerate(pairs, start=1):
        item = WatchlistItem.from_track(track, entry_id, position=position)
        previous = saved.get(entry_id)
        if previous is not None:
            item.stages = previous.stages
            item.last_action = previous.last_action
            item.last_error = previous.last_error
        merged.append(item)
    return merged


def _reconcile_spotify_playlist(
    playlist: WatchlistPlaylist,
    *,
    options: WorkflowOptions,
) -> None:
    """Update Spotify card stages from the saved playlist state."""
    state = load_playlist_state(playlist.state_id)
    for item in playlist.items:
        for stage in item.stages.values():
            if stage.status == StageStatus.RUNNING:
                stage.status = StageStatus.NOT_STARTED
        # A Spotify track is one file: it has no quality check, no chapters
        # to parse, and nothing to split.
        for name in ("quality", "parse", "split"):
            item.stages[name].status = StageStatus.SKIPPED
        entry = state["videos"].get(item.entry_id or "", {})
        status = entry.get("status")
        if status not in {"downloaded", "organized"}:
            continue
        files = entry.get("files") or []
        item.stages["download"] = StageRecord(
            status=StageStatus.COMPLETE,
            path=str(files[0]) if files else None,
        )
        if status == "organized":
            item.stages["organize"].status = StageStatus.COMPLETE
            if item.entry_id and item.entry_id not in playlist.processed_video_ids:
                playlist.processed_video_ids.append(item.entry_id)
        elif options.no_organize:
            item.stages["organize"].status = StageStatus.SKIPPED


def _mark_track_completed(item: WatchlistItem, *, options: WorkflowOptions) -> None:
    updated_at = _now()
    item.last_action = "refresh"
    item.last_error = None
    item.stages["download"].status = StageStatus.COMPLETE
    item.stages["download"].updated_at = updated_at
    for name in ("quality", "parse", "split"):
        item.stages[name].status = StageStatus.SKIPPED
        item.stages[name].updated_at = updated_at
    item.stages["organize"].status = (
        StageStatus.SKIPPED if options.no_organize else StageStatus.COMPLETE
    )
    item.stages["organize"].updated_at = updated_at


def reconcile_watchlist(
    watchlist: Watchlist,
    *,
    request: WorkflowRequest,
    options: WorkflowOptions,
) -> None:
    """Update item stages from muzik records, local files, and Beets."""
    beets_library = _open_beets_library(options.config)
    for playlist in watchlist.playlists:
        if playlist.source_kind is WatchlistSourceKind.SPOTIFY:
            _reconcile_spotify_playlist(playlist, options=options)
            continue
        _reconcile_playlist(
            playlist,
            request=request,
            options=options,
            beets_library=beets_library,
        )


def _open_beets_library(config_path: Path | None) -> Library | None:
    # Best-effort: an unconfigured or broken Beets setup must not break
    # watchlist reconciliation, which already works fine without it.
    try:
        return open_library(config_path)
    except Exception:
        return None


def _merge_playlist_items(
    existing: list[WatchlistItem],
    discovered: list[YouTubePlaylistItem],
) -> list[WatchlistItem]:
    old_by_key: dict[tuple[str, int], WatchlistItem] = {}
    old_occurrences: dict[str, int] = {}
    for item in existing:
        base = item.video_id or f"unavailable:{item.position}"
        occurrence = old_occurrences.get(base, 0)
        old_occurrences[base] = occurrence + 1
        old_by_key[(base, occurrence)] = item

    merged: list[WatchlistItem] = []
    new_occurrences: dict[str, int] = {}
    for discovered_item in discovered:
        base = discovered_item.video_id or f"unavailable:{discovered_item.position}"
        occurrence = new_occurrences.get(base, 0)
        new_occurrences[base] = occurrence + 1
        item = old_by_key.get((base, occurrence))
        if item is None:
            item = WatchlistItem.from_youtube(discovered_item)
        else:
            item.position = discovered_item.position
            item.title = discovered_item.title
            item.video_id = discovered_item.video_id
            item.video_url = discovered_item.video_url
            item.thumbnail_url = discovered_item.thumbnail_url
        merged.append(item)
    return merged


def _reconcile_playlist(
    playlist: WatchlistPlaylist,
    *,
    request: WorkflowRequest,
    options: WorkflowOptions,
    beets_library: Library | None = None,
) -> None:
    playlist_state = load_playlist_state(playlist.playlist_id)
    for item in playlist.items:
        for stage in item.stages.values():
            if stage.status == StageStatus.RUNNING:
                stage.status = StageStatus.NOT_STARTED
        video_id = item.video_id
        if not video_id:
            continue
        entry = playlist_state["videos"].get(video_id, {})
        remaining_target = (
            None
            if options.no_organize
            else _remaining_organize_target(entry, request=request)
        )
        if remaining_target is not None:
            if video_id in playlist.processed_video_ids:
                playlist.processed_video_ids.remove(video_id)
            _mark_organize_failed(item, remaining_target, entry=entry)
            continue
        if video_id in playlist.processed_video_ids:
            _mark_workflow_completed(item, options=options)
            continue
        if not entry:
            entry = backfill_playlist_entry_from_legacy_cache(
                video_id, splits=request.splits
            )
        status = entry.get("status")
        if status in {"downloaded", "split", "organized"}:
            download_path = entry.get("audio_file")
            files = entry.get("files") or []
            if not download_path and files:
                download_path = files[0]
            item.stages["download"] = StageRecord(
                status=StageStatus.COMPLETE,
                path=str(download_path) if download_path else None,
            )
        else:
            local_files = find_audio_by_id(request.output, video_id)
            if local_files:
                item.stages["download"] = StageRecord(
                    status=StageStatus.COMPLETE,
                    path=str(local_files[0].resolve()),
                )
            elif beets_library is not None:
                organized_path = find_path_by_source_id(
                    video_id, beets_library
                ) or find_organized_path(item.title, beets_library)
                if organized_path is not None:
                    # No muzik record of this video exists, but its album is
                    # already in the Beets library — treat it the same as a
                    # real cache hit so the blocks below (and any future
                    # reconcile, via processed_video_ids) short-circuit too.
                    status = "organized"
                    item.stages["download"] = StageRecord(
                        status=StageStatus.COMPLETE,
                        path=str(organized_path),
                    )
        if status in {"split", "organized"}:
            item.stages["parse"].status = StageStatus.COMPLETE
            split_dir = entry.get("split_dir")
            item.stages["split"] = StageRecord(
                status=StageStatus.COMPLETE if split_dir else StageStatus.SKIPPED,
                path=str(split_dir) if split_dir else None,
            )
        if status == "organized":
            item.stages["organize"].status = StageStatus.COMPLETE
            if video_id not in playlist.processed_video_ids:
                playlist.processed_video_ids.append(video_id)


def _remaining_organize_target(
    entry: dict[str, Any],
    *,
    request: WorkflowRequest,
) -> Path | None:
    if entry.get("status") != "organized":
        return None
    audio_value = entry.get("audio_file")
    audio = Path(audio_value) if isinstance(audio_value, str) else None
    split_value = entry.get("split_dir")
    split = Path(split_value) if isinstance(split_value, str) else None
    if split is None and audio is not None:
        expected = request.splits / audio.stem
        if expected.is_dir():
            split = expected
    if split is not None and split.is_dir():
        return split if find_audio_inputs([split]) else None
    if audio is not None and audio.is_file():
        return audio
    files = entry.get("files")
    if isinstance(files, list):
        for value in files:
            if isinstance(value, str) and Path(value).is_file():
                return Path(value)
    return None


def _mark_organize_failed(
    item: WatchlistItem,
    target: Path,
    *,
    entry: dict[str, Any],
) -> None:
    updated_at = _now()
    message = "Beets did not import this item. Select Retry."
    audio_value = entry.get("audio_file")
    item.last_action = "refresh"
    item.last_error = message
    item.stages["download"] = StageRecord(
        status=StageStatus.COMPLETE,
        updated_at=updated_at,
        path=audio_value if isinstance(audio_value, str) else None,
    )
    item.stages["parse"] = StageRecord(
        status=StageStatus.COMPLETE,
        updated_at=updated_at,
    )
    item.stages["split"] = StageRecord(
        status=StageStatus.COMPLETE if target.is_dir() else StageStatus.SKIPPED,
        updated_at=updated_at,
        path=str(target) if target.is_dir() else None,
    )
    item.stages["organize"] = StageRecord(
        status=StageStatus.FAILED,
        updated_at=updated_at,
        error=message,
    )


def _mark_workflow_completed(
    item: WatchlistItem,
    *,
    options: WorkflowOptions,
) -> None:
    updated_at = _now()
    item.last_action = "refresh"
    item.last_error = None
    item.stages["download"].status = StageStatus.COMPLETE
    item.stages["download"].updated_at = updated_at
    item.stages["quality"].status = (
        StageStatus.SKIPPED
        if QualityPolicy(options.quality_policy) == QualityPolicy.OFF
        else StageStatus.COMPLETE
    )
    item.stages["quality"].updated_at = updated_at
    item.stages["parse"].status = (
        StageStatus.SKIPPED if options.no_split else StageStatus.COMPLETE
    )
    item.stages["parse"].updated_at = updated_at
    if options.no_split:
        item.stages["split"].status = StageStatus.SKIPPED
    elif item.stages["split"].status != StageStatus.COMPLETE:
        item.stages["split"].status = StageStatus.SKIPPED
    item.stages["split"].updated_at = updated_at
    item.stages["organize"].status = (
        StageStatus.SKIPPED if options.no_organize else StageStatus.COMPLETE
    )
    item.stages["organize"].updated_at = updated_at


def _now() -> str:
    return datetime.now().astimezone().isoformat(timespec="seconds")


def _watchlist_from_data(raw: object) -> Watchlist:
    root = _mapping(raw, "watchlist")
    version = root.get("version")
    if version not in SUPPORTED_WATCHLIST_VERSIONS:
        raise WatchlistFormatError(
            f"Unsupported watchlist version {version!r}; expected one of "
            f"{SUPPORTED_WATCHLIST_VERSIONS}."
        )
    playlists_raw = root.get("playlists")
    if not isinstance(playlists_raw, list):
        raise WatchlistFormatError("Watchlist field 'playlists' must be a list.")
    return Watchlist(
        version=WATCHLIST_VERSION,
        playlists=[
            _playlist_from_data(value, f"playlists[{index}]")
            for index, value in enumerate(playlists_raw)
        ],
    )


def _playlist_from_data(raw: object, field_name: str) -> WatchlistPlaylist:
    data = _mapping(raw, field_name)
    playlist_id_value = _required_string(data, "playlist_id", field_name)
    url = _required_string(data, "url", field_name)
    items_raw = data.get("items", [])
    if not isinstance(items_raw, list):
        raise WatchlistFormatError(f"{field_name}.items must be a list.")
    processed = data.get("processed_video_ids", [])
    if not isinstance(processed, list) or not all(
        isinstance(value, str) for value in processed
    ):
        raise WatchlistFormatError(
            f"{field_name}.processed_video_ids must be a string list."
        )
    processed_ids = [value for value in processed if isinstance(value, str)]
    raw_kind = data.get("kind", WatchlistSourceKind.YOUTUBE.value)
    try:
        kind = WatchlistSourceKind(raw_kind)
    except (TypeError, ValueError) as exc:
        raise WatchlistFormatError(
            f"{field_name}.kind has unknown value {raw_kind!r}."
        ) from exc
    return WatchlistPlaylist(
        playlist_id=playlist_id_value,
        url=url,
        kind=kind.value,
        title=_optional_string(data, "title", field_name),
        items=[
            _item_from_data(value, f"{field_name}.items[{index}]")
            for index, value in enumerate(items_raw)
        ],
        processed_video_ids=list(dict.fromkeys(processed_ids)),
        last_checked_at=_optional_string(data, "last_checked_at", field_name),
        last_error=_optional_string(data, "last_error", field_name),
    )


def _item_from_data(raw: object, field_name: str) -> WatchlistItem:
    data = _mapping(raw, field_name)
    position = data.get("position")
    if not isinstance(position, int) or isinstance(position, bool) or position < 1:
        raise WatchlistFormatError(f"{field_name}.position must be a positive integer.")
    stages_raw = data.get("stages", {})
    if not isinstance(stages_raw, dict):
        raise WatchlistFormatError(f"{field_name}.stages must be an object.")
    stages = new_stage_records()
    for name in STAGE_NAMES:
        if name in stages_raw:
            stages[name] = _stage_from_data(
                stages_raw[name], f"{field_name}.stages.{name}"
            )
    raw_kind = data.get("kind", WatchlistSourceKind.YOUTUBE.value)
    try:
        kind = WatchlistSourceKind(raw_kind)
    except (TypeError, ValueError) as exc:
        raise WatchlistFormatError(
            f"{field_name}.kind has unknown value {raw_kind!r}."
        ) from exc
    track = data.get("track")
    if track is not None and not isinstance(track, dict):
        raise WatchlistFormatError(f"{field_name}.track must be an object or null.")
    return WatchlistItem(
        position=position,
        title=_required_string(data, "title", field_name),
        video_id=_optional_string(data, "video_id", field_name),
        video_url=_optional_string(data, "video_url", field_name),
        thumbnail_url=_optional_string(data, "thumbnail_url", field_name),
        kind=kind.value,
        entry_id=_optional_string(data, "entry_id", field_name),
        track=cast(dict[str, Any], track) if track is not None else None,
        stages=stages,
        last_action=_optional_string(data, "last_action", field_name),
        last_error=_optional_string(data, "last_error", field_name),
    )


def _stage_from_data(raw: object, field_name: str) -> StageRecord:
    data = _mapping(raw, field_name)
    raw_status = data.get("status", StageStatus.NOT_STARTED.value)
    try:
        status = StageStatus(raw_status)
    except (TypeError, ValueError) as exc:
        raise WatchlistFormatError(
            f"{field_name}.status has unknown value {raw_status!r}."
        ) from exc
    return StageRecord(
        status=status,
        updated_at=_optional_string(data, "updated_at", field_name),
        path=_optional_string(data, "path", field_name),
        error=_optional_string(data, "error", field_name),
    )


def _mapping(raw: object, field_name: str) -> dict[str, Any]:
    if not isinstance(raw, dict):
        raise WatchlistFormatError(f"{field_name} must be an object.")
    if not all(isinstance(key, str) for key in raw):
        raise WatchlistFormatError(f"{field_name} keys must be strings.")
    return cast(dict[str, Any], raw)


def _required_string(data: dict[str, Any], key: str, field_name: str) -> str:
    value = data.get(key)
    if not isinstance(value, str) or not value.strip():
        raise WatchlistFormatError(f"{field_name}.{key} must be a non-empty string.")
    return value


def _optional_string(data: dict[str, Any], key: str, field_name: str) -> str | None:
    value = data.get(key)
    if value is not None and not isinstance(value, str):
        raise WatchlistFormatError(f"{field_name}.{key} must be a string or null.")
    return value

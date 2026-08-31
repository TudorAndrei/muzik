"""Durable YouTube playlist watchlist records."""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass, field
from datetime import datetime
from enum import Enum
import json
import os
from pathlib import Path
import tempfile
from typing import Any, cast

from muzik.config import MUZIK_WATCHLIST_FILE
from muzik.core.sources.youtube import (
    PlaylistLookupError,
    YouTubePlaylistItem,
    find_audio_by_id,
    get_playlist_items,
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
    PlaylistVideoResult,
    WorkflowOptions,
    WorkflowRequest,
    WorkflowRunOperations,
    backfill_playlist_entry_from_legacy_cache,
    load_playlist_state,
    run_youtube_playlist_videos,
)


WATCHLIST_VERSION = 1
STAGE_NAMES = ("download", "parse", "split", "organize")


class WatchlistError(RuntimeError):
    """Base error for watchlist operations."""


class WatchlistFormatError(WatchlistError):
    """Raised when a saved watchlist has invalid data."""


class InvalidPlaylistUrlError(WatchlistError):
    """Raised when a URL does not contain a YouTube playlist ID."""


class DuplicatePlaylistError(WatchlistError):
    """Raised when the watchlist already contains a playlist ID."""


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
    stages: dict[str, StageRecord] = field(default_factory=new_stage_records)
    last_action: str | None = None
    last_error: str | None = None

    @property
    def key(self) -> str:
        return self.video_id or f"unavailable:{self.position}"

    @classmethod
    def from_youtube(cls, item: YouTubePlaylistItem) -> WatchlistItem:
        return cls(
            position=item.position,
            title=item.title,
            video_id=item.video_id,
            video_url=item.video_url,
            thumbnail_url=item.thumbnail_url,
        )

    def to_dict(self) -> dict[str, Any]:
        return {
            "position": self.position,
            "title": self.title,
            "video_id": self.video_id,
            "video_url": self.video_url,
            "thumbnail_url": self.thumbnail_url,
            "stages": {name: record.to_dict() for name, record in self.stages.items()},
            "last_action": self.last_action,
            "last_error": self.last_error,
        }


@dataclass(slots=True)
class WatchlistPlaylist:
    playlist_id: str
    url: str
    items: list[WatchlistItem] = field(default_factory=list)
    processed_video_ids: list[str] = field(default_factory=list)
    last_checked_at: str | None = None
    last_error: str | None = None

    def to_dict(self) -> dict[str, Any]:
        return {
            "playlist_id": self.playlist_id,
            "url": self.url,
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
        normalized_id = playlist_id(url.strip())
        if not normalized_id:
            raise InvalidPlaylistUrlError("Enter a YouTube playlist URL.")
        watchlist = self.load()
        if any(item.playlist_id == normalized_id for item in watchlist.playlists):
            raise DuplicatePlaylistError(
                f"Playlist {normalized_id} is already in the watchlist."
            )
        playlist = WatchlistPlaylist(
            playlist_id=normalized_id,
            url=f"https://www.youtube.com/playlist?list={normalized_id}",
        )
        watchlist.playlists.append(playlist)
        self.save(watchlist)
        return playlist

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
    item_loader: Callable[[str], list[YouTubePlaylistItem]] = get_playlist_items,
    events: WorkflowEventEmitter | None = None,
    cancellation: CancellationToken | None = None,
) -> WatchlistRefreshSummary:
    """Refresh every saved playlist and process only its pending videos."""
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
        events.emit(MessageEvent(f"Checking playlist {playlist.playlist_id}."))
        try:
            discovered = item_loader(playlist.url)
        except PlaylistLookupError as exc:
            playlist.last_checked_at = _now()
            playlist.last_error = str(exc)
            repository.save(watchlist)
            playlist_errors += 1
            events.emit(MessageEvent(str(exc), severity="error"))
            events.emit(ProgressAdvancedEvent(task_id=task_id))
            continue

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
        playlists_checked=len(watchlist.playlists),
        pending_videos=pending_total,
        completed_videos=completed_total,
        failed_videos=failed_total,
        playlist_errors=playlist_errors,
    )


def reconcile_watchlist(
    watchlist: Watchlist,
    *,
    request: WorkflowRequest,
    options: WorkflowOptions,
) -> None:
    """Update item stages from existing muzik records and local files."""
    for playlist in watchlist.playlists:
        _reconcile_playlist(playlist, request=request, options=options)


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
) -> None:
    playlist_state = load_playlist_state(playlist.playlist_id)
    for item in playlist.items:
        for stage in item.stages.values():
            if stage.status == StageStatus.RUNNING:
                stage.status = StageStatus.NOT_STARTED
        video_id = item.video_id
        if not video_id:
            continue
        if video_id in playlist.processed_video_ids:
            _mark_workflow_completed(item, options=options)
            continue
        entry = playlist_state["videos"].get(video_id, {})
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
    if version != WATCHLIST_VERSION:
        raise WatchlistFormatError(
            f"Unsupported watchlist version {version!r}; expected {WATCHLIST_VERSION}."
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
    return WatchlistPlaylist(
        playlist_id=playlist_id_value,
        url=url,
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
    return WatchlistItem(
        position=position,
        title=_required_string(data, "title", field_name),
        video_id=_optional_string(data, "video_id", field_name),
        video_url=_optional_string(data, "video_url", field_name),
        thumbnail_url=_optional_string(data, "thumbnail_url", field_name),
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

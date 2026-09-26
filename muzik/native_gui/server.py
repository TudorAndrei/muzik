"""Transport and core adapters for a GPUI child process."""

from __future__ import annotations

import asyncio
from concurrent.futures import ThreadPoolExecutor
from dataclasses import asdict, fields, is_dataclass
from datetime import datetime
from enum import Enum
import json
from pathlib import Path
import re
import sys
from threading import Event, Lock, Thread
from typing import Any, TextIO
from uuid import uuid4

from muzik.config import (
    DEFAULT_DOWNLOAD_DIR,
    DEFAULT_SPLITS_DIR,
    get_spotify_settings,
    save_muzik_config_value,
)
from muzik.core.beets.decisions import BeetsDuplicateDecision, BeetsMatchDecision
from muzik.core.library import human_size, scan_downloads
from muzik.core.services import check_services
from muzik.core.sources.base import Candidate
from muzik.core.sources.spotify_api import SpotifyClient
from muzik.core.sources.spotify_auth import (
    TokenStore,
    login as spotify_login,
    redirect_uri,
)
from muzik.core.thumbnails import (
    ThumbnailRequest,
    cache_thumbnails,
    cached_thumbnail_path,
)
from muzik.core.watchlist import (
    WatchlistError,
    WatchlistRepository,
    reconcile_watchlist,
    refresh_watchlist,
)
from muzik.core.workflow.cancellation import CancellationToken, WorkflowCancelled
from muzik.core.workflow.decisions import ChapterDecision, WorkflowDecisionError
from muzik.core.workflow.events import WorkflowEvent
from muzik.core.workflow.item_actions import (
    ItemAction,
    item_action_availability,
    item_summary_state,
    primary_item_action,
    run_item_action,
)
from muzik.core.workflow.operations import (
    build_item_action_operations,
    build_workflow_operations,
)
from muzik.core.workflow.service import WorkflowOptions, WorkflowRequest, run_workflow


def _json_value(value: Any) -> Any:
    if isinstance(value, Path):
        return str(value)
    if isinstance(value, Enum):
        return value.value
    if is_dataclass(value) and not isinstance(value, type):
        return {key: _json_value(item) for key, item in asdict(value).items()}
    if isinstance(value, dict):
        return {str(key): _json_value(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [_json_value(item) for item in value]
    return value


def _event_name(event: object) -> str:
    name = type(event).__name__.removesuffix("Event")
    return re.sub(r"(?<!^)(?=[A-Z])", "_", name).lower()


class NativeGuiServer:
    """Own one active workflow and write complete JSON records under a lock."""

    def __init__(
        self,
        reader: TextIO,
        writer: TextIO,
        *,
        repository: WatchlistRepository | None = None,
    ) -> None:
        self.reader = reader
        self.writer = writer
        self.repository = repository or WatchlistRepository()
        self._write_lock = Lock()
        self._job_lock = Lock()
        self._job: Thread | None = None
        self._job_id: str | None = None
        self._cancellation: CancellationToken | None = None
        self._decisions: dict[str, tuple[Event, list[Any]]] = {}
        self._reconcile_worker: Thread | None = None

    def _write(self, record: dict[str, Any]) -> None:
        line = json.dumps(_json_value(record), ensure_ascii=False, allow_nan=False)
        with self._write_lock:
            self.writer.write(line + "\n")
            self.writer.flush()

    def _event(self, name: str, data: dict[str, Any]) -> None:
        self._write({"type": "event", "event": name, "data": data})

    def _respond(self, request_id: Any, command: str, params: dict[str, Any]) -> None:
        try:
            if command == "watchlist.load":
                request, options = _request_options(params)
                watchlist = self.repository.load()
                result = {"watchlist": _watchlist_data(watchlist, request)}
            else:
                result = self.dispatch(command, params)
        except Exception as exc:
            self._write(
                {
                    "id": request_id,
                    "type": "response",
                    "ok": False,
                    "error": {"code": _error_code(exc), "message": str(exc)},
                }
            )
            return
        self._write(
            {"id": request_id, "type": "response", "ok": True, "result": result}
        )
        if command == "watchlist.load":
            # The checked state must not arrive before the saved-state response.
            try:
                self._start_reconcile(request, options)
            except Exception as exc:
                self._event("watchlist.error", {"message": str(exc)})

    def serve(self) -> None:
        with ThreadPoolExecutor(
            max_workers=4, thread_name_prefix="muzik-native-read"
        ) as reads:
            try:
                for line in self.reader:
                    if not line.strip():
                        continue
                    request: Any = None
                    try:
                        request = json.loads(line)
                        if not isinstance(request, dict):
                            raise ValueError("Request must be an object.")
                        request_id = request.get("id")
                        command = request.get("command")
                        params = request.get("params", {})
                        if not isinstance(command, str) or not isinstance(params, dict):
                            raise ValueError(
                                "Request needs a command and object params."
                            )
                    except Exception as exc:
                        self._write(
                            {
                                "id": request.get("id")
                                if isinstance(request, dict)
                                else None,
                                "type": "response",
                                "ok": False,
                                "error": {
                                    "code": _error_code(exc),
                                    "message": str(exc),
                                },
                            }
                        )
                        continue
                    if command in {
                        "library.scan",
                        "services.check",
                        "spotify.status",
                        "spotify.playlists",
                    }:
                        reads.submit(self._respond, request_id, command, params)
                    else:
                        self._respond(request_id, command, params)
            finally:
                self.cancel()
                if self._job is not None:
                    self._job.join(timeout=5)
                if self._reconcile_worker is not None:
                    self._reconcile_worker.join(timeout=5)

    def dispatch(self, command: str, params: dict[str, Any]) -> dict[str, Any]:
        if command == "hello":
            return {
                "protocol_version": 1,
                "defaults": {
                    "output": str(DEFAULT_DOWNLOAD_DIR),
                    "splits": str(DEFAULT_SPLITS_DIR),
                    "audio_source": "youtube",
                    "metadata_source": "auto",
                    "prefer": "lossless",
                    "fallback": "youtube",
                    "quality_policy": "off",
                    "min_bitrate": 256,
                    "jobs": 0,
                    "interactive": True,
                },
                "item_actions": [item.value for item in ItemAction],
            }
        if command == "job.cancel":
            job_id = _required_string(params, "job_id")
            if job_id != self._job_id or self._job is None or not self._job.is_alive():
                raise ValueError("The job is not active.")
            self.cancel()
            return {"job_id": job_id, "cancel_requested": True}
        if command == "decision.reply":
            decision_id = _required_string(params, "decision_id")
            with self._job_lock:
                pending = self._decisions.pop(decision_id, None)
            if pending is None:
                raise ValueError("The decision is not pending.")
            pending[1].append(params.get("value"))
            pending[0].set()
            return {"decision_id": decision_id}
        if command == "services.check":
            return {"services": _json_value(check_services())}
        if command == "library.scan":
            output = _path(params.get("output"), DEFAULT_DOWNLOAD_DIR)
            items = scan_downloads(output)
            return {
                "output": str(output),
                "total_size": human_size(sum(item.size for item in items)),
                "items": [
                    {
                        **_json_value(item),
                        "size_label": human_size(item.size),
                        "modified": datetime.fromtimestamp(item.mtime).strftime(
                            "%Y-%m-%d %H:%M"
                        ),
                    }
                    for item in items
                ],
            }
        if command == "spotify.status":
            settings = get_spotify_settings()
            token_saved = TokenStore().load() is not None
            result: dict[str, Any] = {
                "client_id": settings.get("client_id", ""),
                "redirect_uri": redirect_uri(
                    int(settings.get("redirect_port", "8888") or "8888")
                ),
                "connected": token_saved,
            }
            if token_saved:
                try:
                    result["account_name"] = SpotifyClient().account_name()
                except Exception as exc:
                    result["connected"] = False
                    result["error"] = str(exc)
            return result
        if command == "spotify.set_client_id":
            save_muzik_config_value(
                "spotify", "client_id", _required_string(params, "client_id")
            )
            return {"client_id": get_spotify_settings().get("client_id", "")}
        if command == "spotify.logout":
            return {"removed": TokenStore().clear()}
        if command == "spotify.playlists":
            return {"playlists": _json_value(SpotifyClient().list_playlists())}
        if command == "spotify.login":
            port = params.get("port")
            if port is not None:
                if not isinstance(port, int) or not 1 <= port <= 65535:
                    raise ValueError("port must be an integer from 1 to 65535.")
                save_muzik_config_value("spotify", "redirect_port", str(port))
            return self._start_spotify_login()
        if command == "watchlist.load":
            request, options = _request_options(params)
            watchlist = self.repository.load()
            self._start_reconcile(request, options)
            return {"watchlist": _watchlist_data(watchlist, request)}
        if command == "watchlist.add":
            with self._job_lock:
                self._require_idle()
                playlist = self.repository.add(_required_string(params, "url"))
            return {
                "playlist": playlist.to_dict(),
                "watchlist": _watchlist_data(self.repository.load()),
            }
        if command == "watchlist.rename":
            with self._job_lock:
                self._require_idle()
                renamed = self.repository.rename(
                    _required_string(params, "playlist_id"),
                    _required_string(params, "title"),
                )
            return {
                "renamed": renamed,
                "watchlist": _watchlist_data(self.repository.load()),
            }
        if command == "watchlist.remove":
            with self._job_lock:
                self._require_idle()
                removed = self.repository.remove(
                    _required_string(params, "playlist_id")
                )
            return {
                "removed": removed,
                "watchlist": _watchlist_data(self.repository.load()),
            }
        if command == "thumbnails.cache":
            video_ids = params.get("video_ids")
            if (
                not isinstance(video_ids, list)
                or len(video_ids) > 16
                or any(not isinstance(video_id, str) for video_id in video_ids)
            ):
                raise ValueError("video_ids must be a list of at most 16 IDs.")
            return self._start_thumbnail_cache(set(video_ids))
        if command in {"workflow.start", "watchlist.refresh", "watchlist.action"}:
            request, options = _request_options(params)
            if command == "workflow.start" and not request.raw:
                raise ValueError("Enter a URL or path.")
            if command == "watchlist.refresh" and not self.repository.load().playlists:
                raise ValueError("Add a playlist before you refresh.")
            if command == "watchlist.action":
                _required_string(params, "playlist_id")
                if not isinstance(params.get("position"), int):
                    raise ValueError("position must be an integer.")
                ItemAction(_required_string(params, "action"))
            return self._start_job(command, params, request, options)
        raise ValueError(f"Unknown command: {command}")

    def _job_active(self) -> bool:
        return self._job is not None and self._job.is_alive()

    def _require_idle(self) -> None:
        if self._job_active():
            raise RuntimeError("A job is already active.")

    def _watchlist_stamp(self) -> tuple[int, int]:
        try:
            stat = self.repository.path.stat()
        except OSError:
            return (0, 0)
        return (stat.st_mtime_ns, stat.st_size)

    def _start_reconcile(
        self, request: WorkflowRequest, options: WorkflowOptions
    ) -> None:
        with self._job_lock:
            if self._job_active() or (
                self._reconcile_worker is not None and self._reconcile_worker.is_alive()
            ):
                return
            self._reconcile_worker = Thread(
                target=self._run_reconcile,
                args=(request, options),
                name="muzik-native-gui-reconcile",
                daemon=True,
            )
            self._reconcile_worker.start()

    def _run_reconcile(
        self, request: WorkflowRequest, options: WorkflowOptions
    ) -> None:
        try:
            for _ in range(3):
                stamp = self._watchlist_stamp()
                watchlist = self.repository.load()
                reconcile_watchlist(watchlist, request=request, options=options)
                with self._job_lock:
                    if self._job_active():
                        return
                    if self._watchlist_stamp() != stamp:
                        continue
                    self.repository.save(watchlist)
                    self._event(
                        "watchlist.updated",
                        {"watchlist": _watchlist_data(watchlist, request)},
                    )
                    return
            self._event(
                "watchlist.error",
                {"message": "The watchlist changed during the local check. Reload it."},
            )
        except Exception as exc:
            self._event("watchlist.error", {"message": str(exc)})

    def _start_spotify_login(self) -> dict[str, Any]:
        with self._job_lock:
            if self._job is not None and self._job.is_alive():
                raise RuntimeError("A job is already active.")
            job_id = uuid4().hex
            self._job_id = job_id
            self._cancellation = CancellationToken()
            self._job = Thread(
                target=self._run_spotify_login,
                args=(job_id,),
                name="muzik-spotify-login",
                daemon=True,
            )
            self._job.start()
        return {"job_id": job_id}

    def _run_spotify_login(self, job_id: str) -> None:
        try:
            spotify_login()
            self._event(
                "job.completed",
                {
                    "job_id": job_id,
                    "result": {"account_name": SpotifyClient().account_name()},
                },
            )
        except Exception as exc:
            self._event(
                "job.failed",
                {
                    "job_id": job_id,
                    "error": {"code": _error_code(exc), "message": str(exc)},
                },
            )

    def _start_thumbnail_cache(self, video_ids: set[str]) -> dict[str, Any]:
        with self._job_lock:
            self._require_idle()
            job_id = uuid4().hex
            token = CancellationToken()
            self._job_id = job_id
            self._cancellation = token
            self._job = Thread(
                target=self._run_thumbnail_cache,
                args=(job_id, token, video_ids),
                name="muzik-thumbnail-cache",
                daemon=True,
            )
            self._job.start()
        return {"job_id": job_id}

    def _run_thumbnail_cache(
        self, job_id: str, cancellation: CancellationToken, video_ids: set[str]
    ) -> None:
        try:
            watchlist = self.repository.load()
            requests = {
                item.video_id: ThumbnailRequest(item.video_id, item.thumbnail_url)
                for playlist in watchlist.playlists
                for item in playlist.items
                if item.video_id in video_ids
                and item.thumbnail_url
                and cached_thumbnail_path(item.video_id) is None
            }
            results = asyncio.run(
                cache_thumbnails(requests.values(), cancellation=cancellation)
            )
            cancellation.raise_if_cancelled()
            self._event(
                "job.completed",
                {
                    "job_id": job_id,
                    "result": {
                        "thumbnails": _json_value(results),
                        "watchlist": _watchlist_data(watchlist),
                    },
                },
            )
        except WorkflowCancelled:
            self._event("job.cancelled", {"job_id": job_id})
        except Exception as exc:
            self._event(
                "job.failed",
                {
                    "job_id": job_id,
                    "error": {"code": _error_code(exc), "message": str(exc)},
                },
            )

    def _start_job(
        self,
        command: str,
        params: dict[str, Any],
        request: WorkflowRequest,
        options: WorkflowOptions,
    ) -> dict[str, Any]:
        with self._job_lock:
            if self._job is not None and self._job.is_alive():
                raise RuntimeError("A job is already active.")
            job_id = uuid4().hex
            cancellation = CancellationToken()
            self._job_id = job_id
            self._cancellation = cancellation
            self._job = Thread(
                target=self._run_job,
                args=(job_id, command, params, request, options, cancellation),
                name="muzik-native-gui-job",
                daemon=True,
            )
            self._job.start()
        return {"job_id": job_id}

    def cancel(self) -> None:
        if self._cancellation is not None:
            self._cancellation.cancel()
        with self._job_lock:
            pending = list(self._decisions.values())
            self._decisions.clear()
        for gate, _ in pending:
            gate.set()

    def _run_job(
        self,
        job_id: str,
        command: str,
        params: dict[str, Any],
        request: WorkflowRequest,
        options: WorkflowOptions,
        cancellation: CancellationToken,
    ) -> None:
        emitter = _EventEmitter(self, job_id, "workflow")
        beets_emitter = _EventEmitter(self, job_id, "beets")
        decisions = _WorkflowDecisions(self, job_id, options.interactive, cancellation)
        beets_decisions = _BeetsDecisions(
            self, job_id, options.interactive, cancellation
        )
        try:
            operations = build_workflow_operations(
                splits=request.splits,
                options=options,
                decisions=decisions,
                events=emitter,
                beets_decisions=beets_decisions,
                beets_events=beets_emitter,
            )
            if command == "workflow.start":
                run_workflow(
                    request,
                    options,
                    operations=operations,
                    events=emitter,
                    cancellation=cancellation,
                )
                result: dict[str, Any] = {}
            elif command == "watchlist.refresh":
                summary = refresh_watchlist(
                    self.repository,
                    request,
                    options,
                    operations=operations,
                    events=emitter,
                    cancellation=cancellation,
                )
                result = {
                    "summary": _json_value(summary),
                    "watchlist": _watchlist_data(self.repository.load(), request),
                }
            else:
                result = self._run_item_action(
                    params,
                    request,
                    options,
                    decisions,
                    emitter,
                    beets_decisions,
                    beets_emitter,
                    cancellation,
                )
            cancellation.raise_if_cancelled()
            self._event("job.completed", {"job_id": job_id, "result": result})
        except WorkflowCancelled:
            self._event("job.cancelled", {"job_id": job_id})
        except Exception as exc:
            self._event(
                "job.failed",
                {
                    "job_id": job_id,
                    "error": {"code": _error_code(exc), "message": str(exc)},
                },
            )

    def _run_item_action(
        self,
        params: dict[str, Any],
        request: WorkflowRequest,
        options: WorkflowOptions,
        decisions: Any,
        emitter: Any,
        beets_decisions: Any,
        beets_emitter: Any,
        cancellation: CancellationToken,
    ) -> dict[str, Any]:
        watchlist = self.repository.load()
        playlist = next(
            (
                entry
                for entry in watchlist.playlists
                if entry.playlist_id == params["playlist_id"]
            ),
            None,
        )
        if playlist is None:
            raise WatchlistError("The selected playlist is no longer available.")
        item = next(
            (
                entry
                for entry in playlist.items
                if entry.position == params["position"]
                and entry.video_id == params.get("video_id")
            ),
            None,
        )
        if item is None:
            raise WatchlistError("The selected video is no longer available.")
        action = ItemAction(params["action"])
        item_operations = build_item_action_operations(
            decisions=decisions,
            events=emitter,
            beets_decisions=beets_decisions,
            beets_events=beets_emitter,
        )
        action_result = run_item_action(
            item,
            action,
            request=request,
            options=options,
            operations=item_operations,
            source_id=playlist.playlist_id,
            cancellation=cancellation,
            on_state_change=lambda: self.repository.save(watchlist),
        )
        done_key = item.entry_id or item.video_id
        if (
            done_key
            and item_summary_state(item) == "Processed"
            and done_key not in playlist.processed_video_ids
        ):
            playlist.processed_video_ids.append(done_key)
        self.repository.save(watchlist)
        return {
            "action": _json_value(action_result),
            "watchlist": _watchlist_data(watchlist, request),
        }

    def _request_decision(
        self,
        job_id: str,
        kind: str,
        payload: dict[str, Any],
        cancellation: CancellationToken,
    ) -> Any:
        cancellation.raise_if_cancelled()
        decision_id = uuid4().hex
        gate = Event()
        result: list[Any] = []
        with self._job_lock:
            self._decisions[decision_id] = (gate, result)
        self._event(
            "decision.request",
            {
                "job_id": job_id,
                "decision_id": decision_id,
                "kind": kind,
                "payload": payload,
            },
        )
        while not gate.wait(0.1):
            cancellation.raise_if_cancelled()
        cancellation.raise_if_cancelled()
        if not result:
            raise WorkflowDecisionError("The decision has no response.")
        return result[0]


class _EventEmitter:
    def __init__(self, server: NativeGuiServer, job_id: str, source: str) -> None:
        self.server, self.job_id, self.source = server, job_id, source

    def emit(self, event: WorkflowEvent | Any) -> None:
        self.server._event(
            "job.event",
            {
                "job_id": self.job_id,
                "source": self.source,
                "event": _event_name(event),
                "data": _json_value(event),
            },
        )


class _WorkflowDecisions:
    def __init__(
        self,
        server: NativeGuiServer,
        job_id: str,
        interactive: bool,
        cancellation: CancellationToken,
    ) -> None:
        self.server, self.job_id, self.interactive, self.cancellation = (
            server,
            job_id,
            interactive,
            cancellation,
        )

    def _ask(self, kind: str, payload: dict[str, Any]) -> Any:
        return self.server._request_decision(
            self.job_id, kind, payload, self.cancellation
        )

    def choose_soulseek_candidate(self, candidates: list[Candidate]) -> Candidate:
        if not candidates:
            raise WorkflowDecisionError("No Soulseek candidates available.")
        if not self.interactive:
            return candidates[0]
        value = self._ask("soulseek_candidate", {"candidates": _json_value(candidates)})
        index = value.get("index") if isinstance(value, dict) else value
        if not isinstance(index, int) or not 0 <= index < len(candidates):
            raise WorkflowDecisionError("Select a candidate index in range.")
        return candidates[index]

    def confirm_chapters(self, source: Path, chapters: list[Any]) -> ChapterDecision:
        if not self.interactive:
            return ChapterDecision.ACCEPT
        return ChapterDecision(
            self._ask(
                "chapter_review",
                {"source": str(source), "chapters": _json_value(chapters)},
            )
        )

    def edit_chapters(self, chapters: list[Any]) -> list[Any] | None:
        if not self.interactive:
            return chapters
        value = self._ask("chapter_edit", {"chapters": _json_value(chapters)})
        if value is None:
            return None
        if not isinstance(value, list):
            raise WorkflowDecisionError("Edited chapters must be a list.")
        from muzik.core.chapters import Chapter

        allowed = {field.name for field in fields(Chapter)}
        return [
            Chapter(**{key: item[key] for key in allowed if key in item})
            for item in value
        ]

    def confirm_quality_replacement(self, current: Path, candidate: Candidate) -> bool:
        if not self.interactive:
            return False
        value = self._ask(
            "quality_replacement",
            {"current": str(current), "candidate": candidate.to_dict()},
        )
        if not isinstance(value, bool):
            raise WorkflowDecisionError("Quality replacement needs a boolean reply.")
        return value


class _BeetsDecisions:
    def __init__(
        self,
        server: NativeGuiServer,
        job_id: str,
        interactive: bool,
        cancellation: CancellationToken,
    ) -> None:
        self.server, self.job_id, self.interactive, self.cancellation = (
            server,
            job_id,
            interactive,
            cancellation,
        )

    def should_resume_beets_import(self, path: Path) -> bool:
        return False

    def choose_beets_album_match(self, task: Any) -> str | BeetsMatchDecision | None:
        return self._choose_match(task)

    def choose_beets_track_match(self, task: Any) -> str | BeetsMatchDecision | None:
        return self._choose_match(task)

    def _choose_match(self, task: Any) -> str | BeetsMatchDecision | None:
        if not self.interactive:
            return BeetsMatchDecision.AS_IS
        value = self.server._request_decision(
            self.job_id, "beets_match", {"task": _json_value(task)}, self.cancellation
        )
        if value is None or value == "as_is":
            return BeetsMatchDecision.AS_IS if value == "as_is" else None
        if not isinstance(value, str) or value not in {
            match.candidate_id for match in task.matches
        }:
            raise WorkflowDecisionError("Select a valid Beets match ID.")
        return value

    def resolve_beets_duplicate(
        self, task: Any, duplicates: list[Any]
    ) -> BeetsDuplicateDecision:
        if not self.interactive:
            return BeetsDuplicateDecision.SKIP
        value = self.server._request_decision(
            self.job_id,
            "beets_duplicate",
            {"task": _json_value(task), "duplicates": _json_value(duplicates)},
            self.cancellation,
        )
        return BeetsDuplicateDecision(value)


def _path(value: Any, default: Path) -> Path:
    return (
        Path(value).expanduser()
        if isinstance(value, str) and value.strip()
        else default
    )


def _watchlist_data(
    watchlist: Any, request: WorkflowRequest | None = None
) -> dict[str, Any]:
    request = request or WorkflowRequest("", DEFAULT_DOWNLOAD_DIR, DEFAULT_SPLITS_DIR)
    data = watchlist.to_dict()
    for playlist, playlist_data in zip(
        watchlist.playlists, data["playlists"], strict=True
    ):
        for item, item_data in zip(playlist.items, playlist_data["items"], strict=True):
            cached = cached_thumbnail_path(item.video_id) if item.video_id else None
            item_data["thumbnail_path"] = str(cached) if cached else None
            item_data["summary"] = item_summary_state(item)
            action, label = primary_item_action(item)
            item_data["primary_action"] = (
                {"action": action.value, "label": label} if action else None
            )
            item_data["actions"] = {
                action.value: _json_value(
                    item_action_availability(item, action, request=request)
                )
                for action in ItemAction
            }
    return data


def _required_string(params: dict[str, Any], name: str) -> str:
    value = params.get(name)
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"{name} must be a non-empty string.")
    return value.strip()


def _request_options(params: dict[str, Any]) -> tuple[WorkflowRequest, WorkflowOptions]:
    request = WorkflowRequest(
        raw=str(params.get("raw", "")).strip(),
        output=_path(params.get("output"), DEFAULT_DOWNLOAD_DIR),
        splits=_path(params.get("splits"), DEFAULT_SPLITS_DIR),
    )
    option_names = {field.name for field in fields(WorkflowOptions)}
    values = {name: params[name] for name in option_names if name in params}
    if "config" in values:
        values["config"] = (
            _path(values["config"], Path("")) if values["config"] else None
        )
    return request, WorkflowOptions(**values)


def _error_code(exc: Exception) -> str:
    if isinstance(exc, (ValueError, TypeError)):
        return "invalid_request"
    if isinstance(exc, RuntimeError) and str(exc) == "A job is already active.":
        return "job_active"
    return "operation_failed"


def main() -> None:
    NativeGuiServer(sys.stdin, sys.stdout).serve()

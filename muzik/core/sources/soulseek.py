"""Soulseek source support backed by slskd."""

from __future__ import annotations

import time
from pathlib import Path
from typing import Any

from muzik.config import SLSKD_API_KEY, SLSKD_DOWNLOAD_DIR, SLSKD_URL
from muzik.core import cache as cache_mod
from muzik.core.audio import get_duration
from muzik.core.metadata import write_muzik_metadata
from muzik.core.quality import quality_from_name, score_candidate
from muzik.core.sources.base import (
    Candidate,
    CandidateFile,
    DownloadRequest,
    DownloadResult,
    ResolvedRelease,
    ResolvedTrack,
)


class SoulseekError(RuntimeError):
    """Raised when slskd cannot satisfy a Soulseek request."""


def _load_slskd_api() -> Any:
    try:
        import slskd_api
    except ImportError as exc:
        raise SoulseekError(
            "slskd-api is not installed. Run `uv add slskd-api` and retry."
        ) from exc
    return slskd_api


def _file_name(file_data: dict[str, Any]) -> str:
    return str(file_data.get("filename") or file_data.get("name") or "")


def _response_source_id(response: dict[str, Any]) -> str:
    username = response.get("username") or "unknown"
    token = response.get("token") or ""
    files = response.get("files") or []
    first = _file_name(files[0]) if files else ""
    return f"{username}:{token}:{first}"


def _candidate_file(file_data: dict[str, Any]) -> CandidateFile:
    name = _file_name(file_data)
    quality = quality_from_name(
        file_data.get("extension") or name,
        bitrate=file_data.get("bitRate"),
        sample_rate=file_data.get("sampleRate"),
        size=file_data.get("size"),
    )
    quality.bit_depth = file_data.get("bitDepth")
    return CandidateFile(
        name=name,
        path=name,
        size=file_data.get("size"),
        duration=file_data.get("length"),
        quality=quality,
    )


def candidate_from_response(
    response: dict[str, Any],
    *,
    query: str,
    prefer: str | None = "lossless",
    expected_track_count: int | None = None,
) -> Candidate:
    files_raw = response.get("files") or []
    files = [_candidate_file(file_data) for file_data in files_raw]
    title = query
    if files:
        common_path = Path(files[0].name).parent
        if str(common_path) not in ("", "."):
            title = common_path.name

    candidate = Candidate(
        source="soulseek",
        source_id=_response_source_id(response),
        title=title,
        user=response.get("username"),
        path=str(Path(files[0].name).parent) if files else None,
        files=files,
        metadata={
            "query": query,
            "queueLength": response.get("queueLength"),
            "uploadSpeed": response.get("uploadSpeed"),
            "hasFreeUploadSlot": response.get("hasFreeUploadSlot"),
            "raw_files": files_raw,
        },
    )
    if files:
        candidate.quality = max(files, key=lambda file: file.quality.lossless).quality
    candidate.score = score_candidate(
        candidate,
        prefer=prefer,
        expected_track_count=expected_track_count,
        query=query,
    )
    return candidate


class SoulseekSource:
    """Soulseek source implementation using a running slskd instance."""

    name = "soulseek"

    def __init__(
        self,
        *,
        url: str = SLSKD_URL,
        api_key: str = SLSKD_API_KEY,
        download_dir: str | Path = SLSKD_DOWNLOAD_DIR,
        timeout: float = 30,
    ) -> None:
        self.url = url.rstrip("/")
        self.api_key = api_key
        self.download_dir = Path(download_dir).expanduser()
        self.timeout = timeout
        self._client: Any = None

    @property
    def client(self) -> Any:
        if self._client is None:
            if not self.api_key:
                raise SoulseekError("SLSKD_API_KEY is not configured.")
            slskd_api = _load_slskd_api()
            self._client = slskd_api.SlskdClient(
                self.url,
                api_key=self.api_key,
                timeout=self.timeout,
            )
        return self._client

    def check(self) -> dict[str, Any]:
        """Return application/session state from slskd."""
        state = self.client.application.state()
        auth_valid = self.client.session.auth_valid()
        server = state.get("server") if isinstance(state, dict) else {}
        if not isinstance(server, dict):
            server = {}
        return {
            "url": self.url,
            "download_dir": str(self.download_dir),
            "auth_valid": auth_valid,
            "server_state": server.get("state"),
            "server_connected": bool(server.get("isConnected")),
            "server_logged_in": bool(server.get("isLoggedIn")),
            "state": state,
        }

    def resolve(self, request: DownloadRequest) -> ResolvedRelease | ResolvedTrack:
        raw = request.raw.strip()
        if " - " in raw:
            artist, title = raw.split(" - ", 1)
            return ResolvedRelease(
                title=title.strip(),
                artist=artist.strip(),
                album=title.strip() if request.album is not False else None,
                source=self.name,
                source_id=raw,
                source_metadata={"query": raw},
            )
        return ResolvedTrack(
            title=raw,
            source=self.name,
            source_id=raw,
            source_metadata={"query": raw},
        )

    def search(
        self,
        resolved: ResolvedRelease | ResolvedTrack,
        *,
        prefer: str | None = "lossless",
        limit: int = 20,
        search_timeout: int = 15_000,
    ) -> list[Candidate]:
        query = _query_for_resolved(resolved, prefer)
        self._ensure_server_ready()
        state = self._search_text_with_conflict_retry(
            query,
            limit=limit,
            search_timeout=search_timeout,
        )
        search_id = state.get("id")
        if not search_id:
            raise SoulseekError("slskd did not return a search id.")

        responses = self._search_responses(
            search_id,
            timeout=max(search_timeout / 1000, 1),
        )
        expected_track_count = (
            len(resolved.tracks) if isinstance(resolved, ResolvedRelease) else None
        )
        candidates = [
            candidate_from_response(
                response,
                query=query,
                prefer=prefer,
                expected_track_count=expected_track_count,
            )
            for response in responses
        ]
        return sorted(candidates, key=lambda candidate: candidate.score, reverse=True)

    def _search_text_with_conflict_retry(
        self,
        query: str,
        *,
        limit: int,
        search_timeout: int,
    ) -> dict[str, Any]:
        try:
            return self.client.searches.search_text(
                query,
                responseLimit=limit,
                searchTimeout=search_timeout,
            )
        except Exception as exc:
            if not _is_http_conflict(exc):
                raise
            if not self._delete_completed_searches(query):
                raise
            return self.client.searches.search_text(
                query,
                responseLimit=limit,
                searchTimeout=search_timeout,
            )

    def _search_responses(
        self, search_id: str, *, timeout: float
    ) -> list[dict[str, Any]]:
        deadline = time.monotonic() + timeout
        responses: list[dict[str, Any]] = []

        while time.monotonic() <= deadline:
            try:
                state = self.client.searches.state(search_id, includeResponses=True)
            except Exception:
                break
            raw_responses = state.get("responses") or []
            responses = [
                response for response in raw_responses if isinstance(response, dict)
            ]
            if responses or state.get("isComplete"):
                return responses
            time.sleep(0.5)

        raw_responses = self.client.searches.search_responses(search_id)
        return [response for response in raw_responses if isinstance(response, dict)]

    def _ensure_server_ready(self) -> None:
        info = self.check()
        if not info["auth_valid"]:
            raise SoulseekError("slskd API authentication failed.")
        if not info["server_connected"] or not info["server_logged_in"]:
            state = info.get("server_state") or "unknown"
            raise SoulseekError(
                "slskd API is reachable, but it is not connected and logged in "
                f"to the Soulseek server (state: {state}). Configure "
                "soulseek.username and soulseek.password in slskd.yml, then "
                "restart slskd."
            )

    def _delete_completed_searches(self, query: str) -> bool:
        deleted = False
        for search in self.client.searches.get_all():
            if not isinstance(search, dict):
                continue
            if search.get("searchText") != query:
                continue
            state = str(search.get("state") or "")
            complete = bool(search.get("isComplete"))
            if not complete and "Completed" not in state and "Errored" not in state:
                continue
            search_id = search.get("id")
            if search_id and self.client.searches.delete(search_id):
                deleted = True
        return deleted

    def download(
        self,
        candidate: Candidate,
        output: Path | None = None,
        *,
        wait: bool = True,
        timeout: float = 600,
        queue_timeout: float = 120,
        verify: bool = True,
    ) -> DownloadResult:
        if not candidate.user:
            raise SoulseekError("Cannot download a Soulseek candidate without a user.")
        raw_files = candidate.metadata.get("raw_files") or []
        if not raw_files:
            raise SoulseekError("Cannot download a Soulseek candidate without files.")

        ok = self.client.transfers.enqueue(candidate.user, raw_files)
        if not ok:
            raise SoulseekError("slskd refused the download enqueue request.")

        if wait:
            self._wait_for_candidate(
                candidate,
                timeout=timeout,
                queue_timeout=queue_timeout,
            )

        root = Path(output).expanduser() if output else self.download_dir
        files = self._find_downloaded_files(root, candidate)
        if verify:
            files = _verified_audio_files(files)
            if not files:
                raise SoulseekError("Downloaded files failed ffprobe verification.")
        metadata_target = files[0] if len(files) == 1 else root
        metadata_path = write_muzik_metadata(
            metadata_target,
            {
                "source": self.name,
                "source_id": candidate.source_id,
                "requested": candidate.metadata.get("query") or candidate.title,
                "resolved": {
                    "title": candidate.title,
                    "artist": None,
                    "album": candidate.title,
                    "year": None,
                    "tracks": [],
                },
                "candidate": candidate.to_dict(),
            },
        )
        cache_mod.set(
            cache_mod.download_cache_key(self.name, candidate.source_id),
            str(metadata_target.resolve()),
        )
        return DownloadResult(
            source=self.name,
            source_id=candidate.source_id,
            files=files,
            root=root,
            metadata_path=metadata_path,
            metadata=candidate.metadata,
        )

    def _wait_for_candidate(
        self,
        candidate: Candidate,
        *,
        timeout: float,
        queue_timeout: float,
    ) -> None:
        assert candidate.user is not None
        deadline = time.monotonic() + timeout
        queued_since: float | None = None
        wanted = {Path(file.name).name for file in candidate.files}
        terminal = {"Succeeded", "Completed", "Errored", "Cancelled", "Canceled"}
        queued_states = {"Queued", "Initializing", "Requested", "Pending"}

        while time.monotonic() < deadline:
            transfer = self.client.transfers.get_downloads(candidate.user)
            files = _transfer_files(transfer)
            matching = [
                file
                for file in files
                if Path(str(file.get("filename") or "")).name in wanted
            ]
            if matching and all(
                str(file.get("state")) in queued_states for file in matching
            ):
                queued_since = queued_since or time.monotonic()
                if time.monotonic() - queued_since >= queue_timeout:
                    raise SoulseekError(
                        "Soulseek transfers stayed queued for too long."
                    )
            else:
                queued_since = None
            if matching and all(
                str(file.get("state")) in terminal for file in matching
            ):
                failures = [
                    file
                    for file in matching
                    if str(file.get("state")) not in {"Succeeded", "Completed"}
                ]
                if failures:
                    raise SoulseekError("One or more Soulseek transfers failed.")
                return
            time.sleep(2)
        raise SoulseekError("Timed out waiting for Soulseek transfers to finish.")

    def _find_downloaded_files(self, root: Path, candidate: Candidate) -> list[Path]:
        if not root.exists():
            return []
        wanted = {Path(file.name).name for file in candidate.files}
        return sorted(
            path for path in root.rglob("*") if path.is_file() and path.name in wanted
        )


def _query_for_resolved(
    resolved: ResolvedRelease | ResolvedTrack,
    prefer: str | None,
) -> str:
    parts = [resolved.artist, resolved.album, resolved.title]
    query = " ".join(part for part in parts if part)
    tokens = {token.lower() for token in query.split()}
    if prefer and prefer not in {"any", "lossless"} and prefer.lower() not in tokens:
        query = f"{query} {prefer}"
    elif prefer == "lossless" and not tokens.intersection({"flac", "lossless"}):
        query = f"{query} flac"
    return query.strip()


def _transfer_files(transfer: dict[str, Any]) -> list[dict[str, Any]]:
    files: list[dict[str, Any]] = []
    for directory in transfer.get("directories") or []:
        files.extend(directory.get("files") or [])
    return files


def _verified_audio_files(files: list[Path]) -> list[Path]:
    verified: list[Path] = []
    for file in files:
        duration = get_duration(file)
        if duration is not None and duration > 0:
            verified.append(file)
    return verified


def _is_http_conflict(exc: Exception) -> bool:
    response = getattr(exc, "response", None)
    return getattr(response, "status_code", None) == 409

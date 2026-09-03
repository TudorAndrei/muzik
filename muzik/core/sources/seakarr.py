"""Soulseek source support backed by the embedded Seakarr (soulseek-rs) bridge.

The Soulseek network's remote paths use Windows-style backslashes regardless
of the host platform, so this module normalizes them itself rather than
relying on ``pathlib.Path`` (which only splits on ``/`` on POSIX).
"""

from __future__ import annotations

import re
import time
from pathlib import Path
from typing import Any, Protocol

from muzik.config import (
    SEAKARR_DOWNLOAD_DIR,
    SEAKARR_DOWNLOAD_TIMEOUT,
    SEAKARR_LISTEN_PORT,
    SEAKARR_PASSWORD,
    SEAKARR_SEARCH_LIMIT,
    SEAKARR_SEARCH_TIMEOUT,
    SEAKARR_SERVER_HOST,
    SEAKARR_SERVER_PORT,
    SEAKARR_USERNAME,
)
from muzik.core import cache as cache_mod
from muzik.core.metadata import write_muzik_metadata
from muzik.core.quality import quality_from_name, score_candidate
from muzik.core.workflow.cancellation import CancellationToken
from muzik.core.sources.base import (
    Candidate,
    CandidateFile,
    DownloadRequest,
    DownloadResult,
    ResolvedRelease,
    ResolvedTrack,
)


class SoulseekError(RuntimeError):
    """Raised when Seakarr cannot satisfy a Soulseek request."""


class SeakarrJobProtocol(Protocol):
    def poll(self) -> str: ...
    def cancel(self) -> None: ...
    def result(self) -> Any: ...


class SeakarrSessionProtocol(Protocol):
    def start_track_search(
        self, query: str, timeout_secs: float
    ) -> SeakarrJobProtocol: ...

    def start_download(
        self, username: str, filename: str, size: int, destination: str
    ) -> SeakarrJobProtocol: ...

    def close(self) -> None: ...


def remote_basename(remote_path: str) -> str:
    """Return the last path segment of a Soulseek remote path.

    Soulseek remote paths conventionally use backslashes (Windows clients
    predominate on the network) even when Muzik runs on macOS or Linux, so
    this cannot use ``pathlib.Path`` — POSIX paths do not treat ``\\`` as a
    separator.
    """
    return remote_path.replace("\\", "/").rsplit("/", 1)[-1]


def remote_parent(remote_path: str) -> str:
    """Return everything before the last path segment, or ``""`` at the root."""
    normalized = remote_path.replace("\\", "/")
    if "/" not in normalized:
        return ""
    return normalized.rsplit("/", 1)[0]


def _load_seakarr_module() -> Any:
    try:
        from muzik import _seakarr
    except ImportError as exc:
        raise SoulseekError(
            "The embedded Seakarr bridge is not built. Run `maturin develop` "
            "in rust/seakarr_bridge/ and retry."
        ) from exc
    return _seakarr


def _candidate_file(file_data: dict[str, Any]) -> CandidateFile:
    name = str(file_data.get("name") or "")
    quality = quality_from_name(name)
    bitrate = file_data.get("bitrate_kbps")
    if bitrate is not None:
        quality.bitrate = int(bitrate)
    sample_rate = file_data.get("sample_rate_hz")
    if sample_rate is not None:
        quality.sample_rate = int(sample_rate)
    bit_depth = file_data.get("bit_depth")
    if bit_depth is not None:
        quality.bit_depth = int(bit_depth)
    quality.size = file_data.get("size")
    return CandidateFile(
        name=name,
        path=name,
        size=file_data.get("size"),
        duration=file_data.get("duration_seconds"),
        quality=quality,
    )


def candidate_from_result(
    result: dict[str, Any],
    *,
    query: str,
    prefer: str | None = "lossless",
    expected_track_count: int | None = None,
) -> Candidate:
    """Convert one ``SeakarrJob.result()`` search entry into a ``Candidate``."""
    files_raw = result.get("files") or []
    files = [_candidate_file(file_data) for file_data in files_raw]
    username = str(result.get("username") or "unknown")
    title = query
    if files:
        parent = remote_parent(files[0].name)
        if parent:
            title = remote_basename(parent)

    candidate = Candidate(
        source="soulseek",
        source_id=f"{username}:{files[0].name if files else query}",
        title=title,
        user=result.get("username"),
        path=remote_parent(files[0].name) if files else None,
        files=files,
        metadata={
            "query": query,
            "slots": result.get("slots"),
            "speed": result.get("speed"),
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


class SeakarrSource:
    """Soulseek source implementation using the embedded Seakarr bridge."""

    name = "soulseek"

    def __init__(
        self,
        *,
        username: str = SEAKARR_USERNAME,
        password: str = SEAKARR_PASSWORD,
        server_host: str = SEAKARR_SERVER_HOST,
        server_port: int = SEAKARR_SERVER_PORT,
        listen_port: int = SEAKARR_LISTEN_PORT,
        download_dir: str | Path = SEAKARR_DOWNLOAD_DIR,
        search_limit: int = SEAKARR_SEARCH_LIMIT,
        search_timeout: float = SEAKARR_SEARCH_TIMEOUT,
        download_timeout: float = SEAKARR_DOWNLOAD_TIMEOUT,
    ) -> None:
        self.username = username
        self.password = password
        self.server_host = server_host
        self.server_port = server_port
        self.listen_port = listen_port
        self.download_dir = Path(download_dir).expanduser()
        self.search_limit = search_limit
        self.search_timeout = search_timeout
        self.download_timeout = download_timeout
        self._session: SeakarrSessionProtocol | None = None

    @property
    def session(self) -> SeakarrSessionProtocol:
        if self._session is None:
            if not self.username or not self.password:
                raise SoulseekError("Soulseek username/password are not configured.")
            module = _load_seakarr_module()
            try:
                self._session = module.SeakarrSession.connect(
                    self.username,
                    self.password,
                    server_host=self.server_host,
                    server_port=self.server_port,
                    listen_port=self.listen_port,
                )
            except Exception as exc:
                # Never let a connection failure's message leak the password;
                # SeakarrError text is bridge-generated and never echoes it.
                raise SoulseekError(f"Seakarr connection failed: {exc}") from exc
        return self._session

    def check(self) -> dict[str, Any]:
        """Return connection state. Never includes the password."""
        detail = "Connected"
        connected = True
        try:
            _ = self.session
        except SoulseekError as exc:
            detail = str(exc)
            connected = False
        return {
            "username": self.username,
            "server": f"{self.server_host}:{self.server_port}",
            "download_dir": str(self.download_dir),
            "connected": connected,
            "detail": detail,
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
        search_timeout: float | None = None,
    ) -> list[Candidate]:
        query = _query_for_resolved(resolved, prefer)
        timeout = search_timeout if search_timeout is not None else self.search_timeout
        job = self.session.start_track_search(query, timeout)
        self._wait_for_job(job, timeout=timeout + 5)
        try:
            results = job.result()
        except Exception as exc:
            raise SoulseekError(f"Soulseek search failed: {exc}") from exc

        expected_track_count = (
            len(resolved.tracks) if isinstance(resolved, ResolvedRelease) else None
        )
        candidates = [
            candidate_from_result(
                result,
                query=query,
                prefer=prefer,
                expected_track_count=expected_track_count,
            )
            for result in results
        ]
        candidates.sort(key=lambda candidate: candidate.score, reverse=True)
        return candidates[:limit]

    def _wait_for_job(
        self,
        job: SeakarrJobProtocol,
        *,
        timeout: float,
        cancellation: CancellationToken | None = None,
    ) -> None:
        deadline = time.monotonic() + timeout
        while job.poll() == "running":
            if cancellation is not None:
                if cancellation.is_cancelled():
                    job.cancel()
                cancellation.raise_if_cancelled()
            if time.monotonic() > deadline:
                job.cancel()
                raise SoulseekError("Timed out waiting for a Soulseek job.")
            time.sleep(0.2)

    def download(
        self,
        candidate: Candidate,
        output: Path | None = None,
        *,
        wait: bool = True,
        timeout: float | None = None,
        cancellation: CancellationToken | None = None,
    ) -> DownloadResult:
        cancellation = cancellation or CancellationToken()
        cancellation.raise_if_cancelled()
        if not candidate.user:
            raise SoulseekError("Cannot download a Soulseek candidate without a user.")
        if not candidate.files:
            raise SoulseekError("Cannot download a Soulseek candidate without files.")

        root = Path(output).expanduser() if output else self.download_dir
        root.mkdir(parents=True, exist_ok=True)
        download_timeout = timeout if timeout is not None else self.download_timeout

        files: list[Path] = []
        for file in candidate.files:
            cancellation.raise_if_cancelled()
            job = self.session.start_download(
                candidate.user, file.name, file.size or 0, str(root)
            )
            if not wait:
                continue
            self._wait_for_job(job, timeout=download_timeout, cancellation=cancellation)
            try:
                progress = job.result()
            except Exception as exc:
                raise SoulseekError(f"Soulseek download failed: {exc}") from exc
            if progress.get("state") != "completed":
                reason = progress.get("reason") or progress.get("state")
                raise SoulseekError(f"Soulseek download failed: {reason}")
            local_path = root / remote_basename(file.name)
            if local_path.exists():
                files.append(local_path)

        if wait and not files:
            raise SoulseekError("No downloaded files were found after transfer.")

        metadata_path = None
        if files:
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


# Below what fraction of a query's artist/title words a candidate must match
# in its title, path, and username to count as identity evidence. Chosen to
# tolerate one missing/reordered word (e.g. a peer's folder dropping "The")
# without accepting a same-length coincidence.
_IDENTITY_TOKEN_OVERLAP_THRESHOLD = 0.5

# Default duration tolerance for a direct Spotify-to-Soulseek match. Wide
# enough to absorb a few seconds of silence-trim or encoder rounding between
# Spotify's reported duration and a peer's file, tight enough to reject a
# same-title live/remix/extended version.
DEFAULT_DURATION_TOLERANCE_SECONDS = 10.0


def candidate_matches_track(
    candidate: Candidate,
    track: ResolvedTrack,
    *,
    duration_tolerance_seconds: float = DEFAULT_DURATION_TOLERANCE_SECONDS,
) -> bool:
    """Identity check for direct Spotify-track acquisition.

    Rejects a candidate whose best-matching file duration falls outside
    *duration_tolerance_seconds* of the known track duration, or whose
    title/path/username text does not plausibly reference the track's
    artist and title. A quality upgrade with the wrong recording is worse
    than no upgrade, so this errs toward rejecting weak evidence.
    """
    track_duration = track.duration
    if track_duration and candidate.files:
        durations = [file.duration for file in candidate.files if file.duration]
        if durations:
            closest = min(
                durations, key=lambda duration: abs(duration - track_duration)
            )
            if abs(closest - track_duration) > duration_tolerance_seconds:
                return False

    needed = {
        token
        for token in re.findall(
            r"[a-z0-9]+", f"{track.artist or ''} {track.title}".lower()
        )
        if len(token) > 2
    }
    if not needed:
        return True
    haystack = " ".join(
        filter(None, [candidate.title, candidate.path or "", candidate.user or ""])
    ).lower()
    found = set(re.findall(r"[a-z0-9]+", haystack))
    overlap = len(needed & found) / len(needed)
    return overlap >= _IDENTITY_TOKEN_OVERLAP_THRESHOLD

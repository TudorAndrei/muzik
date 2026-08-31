"""Download and cache watchlist thumbnails."""

from __future__ import annotations

import asyncio
from collections.abc import Awaitable, Callable, Iterable
from dataclasses import dataclass
from pathlib import Path
import re
import tempfile
from urllib.parse import urlsplit

import aiohttp

from muzik.config import CACHE_DIR
from muzik.core.workflow.cancellation import CancellationToken, WorkflowCancelled


_VIDEO_ID_RE = re.compile(r"^[A-Za-z0-9_-]{11}$")
_CONTENT_EXTENSIONS = {
    "image/jpeg": "jpg",
    "image/png": "png",
}


class ThumbnailError(RuntimeError):
    """Raised when a thumbnail cannot be validated or saved."""


@dataclass(frozen=True, slots=True)
class ThumbnailRequest:
    video_id: str
    url: str


@dataclass(frozen=True, slots=True)
class ThumbnailResult:
    video_id: str
    path: Path | None
    cached: bool = False
    error: str | None = None


ThumbnailFetcher = Callable[[str], Awaitable[tuple[bytes, str]]]


def cached_thumbnail_path(
    video_id: str,
    *,
    cache_dir: Path | None = None,
) -> Path | None:
    """Return a cached JPEG or PNG path for one video ID."""
    if not _VIDEO_ID_RE.fullmatch(video_id):
        return None
    root = cache_dir if cache_dir is not None else CACHE_DIR
    for extension in ("jpg", "png"):
        path = root / f"yt_thumbnail_{video_id}.{extension}"
        if path.is_file() and path.stat().st_size > 0:
            return path
    return None


async def cache_thumbnails(
    requests: Iterable[ThumbnailRequest],
    *,
    cache_dir: Path | None = None,
    concurrency: int = 4,
    cancellation: CancellationToken | None = None,
    fetcher: ThumbnailFetcher | None = None,
) -> list[ThumbnailResult]:
    """Cache missing thumbnails with a bounded number of active requests."""
    root = cache_dir if cache_dir is not None else CACHE_DIR
    token = cancellation or CancellationToken()
    request_list = list(requests)
    if concurrency < 1:
        raise ValueError("Thumbnail concurrency must be at least 1.")

    if fetcher is not None:
        return await _cache_with_fetcher(
            request_list,
            root=root,
            concurrency=concurrency,
            cancellation=token,
            fetcher=fetcher,
        )

    timeout = aiohttp.ClientTimeout(total=30)
    async with aiohttp.ClientSession(timeout=timeout) as session:

        async def fetch(url: str) -> tuple[bytes, str]:
            async with session.get(url) as response:
                response.raise_for_status()
                return await response.read(), response.headers.get("Content-Type", "")

        return await _cache_with_fetcher(
            request_list,
            root=root,
            concurrency=concurrency,
            cancellation=token,
            fetcher=fetch,
        )


async def _cache_with_fetcher(
    requests: list[ThumbnailRequest],
    *,
    root: Path,
    concurrency: int,
    cancellation: CancellationToken,
    fetcher: ThumbnailFetcher,
) -> list[ThumbnailResult]:
    semaphore = asyncio.Semaphore(concurrency)

    async def cache_one(request: ThumbnailRequest) -> ThumbnailResult:
        cancellation.raise_if_cancelled()
        if not _VIDEO_ID_RE.fullmatch(request.video_id):
            return ThumbnailResult(
                video_id=request.video_id,
                path=None,
                error="Invalid YouTube video ID.",
            )
        existing = cached_thumbnail_path(request.video_id, cache_dir=root)
        if existing is not None:
            return ThumbnailResult(request.video_id, existing, cached=True)
        try:
            async with semaphore:
                cancellation.raise_if_cancelled()
                data, content_type = await fetcher(_download_url(request))
                cancellation.raise_if_cancelled()
            extension = _validated_extension(data, content_type)
            path = root / f"yt_thumbnail_{request.video_id}.{extension}"
            _atomic_write(path, data)
            return ThumbnailResult(request.video_id, path)
        except WorkflowCancelled:
            raise
        except Exception as exc:
            return ThumbnailResult(
                video_id=request.video_id,
                path=None,
                error=str(exc),
            )

    return list(await asyncio.gather(*(cache_one(request) for request in requests)))


def _download_url(request: ThumbnailRequest) -> str:
    hostname = urlsplit(request.url).hostname
    if hostname in {"i.ytimg.com", "img.youtube.com"}:
        return f"https://i.ytimg.com/vi/{request.video_id}/hqdefault.jpg"
    return request.url


def _validated_extension(data: bytes, content_type: str) -> str:
    media_type = content_type.split(";", maxsplit=1)[0].strip().lower()
    extension = _CONTENT_EXTENSIONS.get(media_type)
    if extension is None:
        raise ThumbnailError("Thumbnail response is not a JPEG or PNG image.")
    if extension == "jpg" and not data.startswith(b"\xff\xd8\xff"):
        raise ThumbnailError("Thumbnail response has invalid JPEG data.")
    if extension == "png" and not data.startswith(b"\x89PNG\r\n\x1a\n"):
        raise ThumbnailError("Thumbnail response has invalid PNG data.")
    return extension


def _atomic_write(path: Path, data: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="wb",
            dir=path.parent,
            prefix=f".{path.name}.",
            suffix=".tmp",
            delete=False,
        ) as handle:
            temporary = Path(handle.name)
            handle.write(data)
            handle.flush()
        temporary.replace(path)
    except OSError as exc:
        raise ThumbnailError(f"Unable to save thumbnail {path}: {exc}") from exc
    finally:
        if temporary is not None and temporary.exists():
            temporary.unlink()

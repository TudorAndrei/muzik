import asyncio
from pathlib import Path

import pytest

from muzik.core.thumbnails import (
    ThumbnailRequest,
    cache_thumbnails,
    cached_thumbnail_path,
)
from muzik.core.workflow.cancellation import CancellationToken, WorkflowCancelled


JPEG = b"\xff\xd8\xffthumbnail"
PNG = b"\x89PNG\r\n\x1a\nthumbnail"


def test_cache_thumbnails_saves_valid_images_atomically(
    tmp_path: Path,
    monkeypatch,
) -> None:
    calls: list[str] = []
    replacements: list[tuple[Path, Path]] = []
    original_replace = Path.replace

    async def fetch(url: str) -> tuple[bytes, str]:
        calls.append(url)
        return JPEG, "image/jpeg; charset=binary"

    def replace(source: Path, target: Path) -> Path:
        replacements.append((source, target))
        return original_replace(source, target)

    monkeypatch.setattr(Path, "replace", replace)
    results = asyncio.run(
        cache_thumbnails(
            [ThumbnailRequest("abcdefghijk", "https://img.test/a.jpg")],
            cache_dir=tmp_path,
            fetcher=fetch,
        )
    )

    expected = tmp_path / "yt_thumbnail_abcdefghijk.jpg"
    assert results[0].path == expected
    assert expected.read_bytes() == JPEG
    assert calls == ["https://img.test/a.jpg"]
    assert replacements[0][0].parent == tmp_path
    assert replacements[0][1] == expected
    assert list(tmp_path.glob("*.tmp")) == []


def test_cache_thumbnails_uses_existing_cache_without_fetch(tmp_path: Path) -> None:
    existing = tmp_path / "yt_thumbnail_abcdefghijk.png"
    existing.write_bytes(PNG)

    async def fail_fetch(url: str) -> tuple[bytes, str]:
        raise AssertionError("cache hit must not fetch")

    result = asyncio.run(
        cache_thumbnails(
            [ThumbnailRequest("abcdefghijk", "https://img.test/a.png")],
            cache_dir=tmp_path,
            fetcher=fail_fetch,
        )
    )[0]

    assert result.path == existing
    assert result.cached is True
    assert cached_thumbnail_path("abcdefghijk", cache_dir=tmp_path) == existing


@pytest.mark.parametrize(
    ("data", "content_type", "message"),
    [
        (b"html", "text/html", "not a JPEG or PNG"),
        (b"bad", "image/jpeg", "invalid JPEG"),
        (b"bad", "image/png", "invalid PNG"),
    ],
)
def test_cache_thumbnails_rejects_invalid_responses(
    tmp_path: Path,
    data: bytes,
    content_type: str,
    message: str,
) -> None:
    async def fetch(url: str) -> tuple[bytes, str]:
        return data, content_type

    result = asyncio.run(
        cache_thumbnails(
            [ThumbnailRequest("abcdefghijk", "https://img.test/a")],
            cache_dir=tmp_path,
            fetcher=fetch,
        )
    )[0]

    assert result.path is None
    assert result.error is not None and message in result.error


def test_failed_thumbnail_retries_on_later_call(tmp_path: Path) -> None:
    attempts = 0

    async def fetch(url: str) -> tuple[bytes, str]:
        nonlocal attempts
        attempts += 1
        if attempts == 1:
            raise RuntimeError("temporary image error")
        return PNG, "image/png"

    request = ThumbnailRequest("abcdefghijk", "https://img.test/a.png")
    first = asyncio.run(cache_thumbnails([request], cache_dir=tmp_path, fetcher=fetch))[
        0
    ]
    second = asyncio.run(
        cache_thumbnails([request], cache_dir=tmp_path, fetcher=fetch)
    )[0]

    assert first.path is None
    assert first.error == "temporary image error"
    assert second.path == tmp_path / "yt_thumbnail_abcdefghijk.png"
    assert attempts == 2


def test_thumbnail_concurrency_is_bounded(tmp_path: Path) -> None:
    active = 0
    maximum = 0

    async def fetch(url: str) -> tuple[bytes, str]:
        nonlocal active, maximum
        active += 1
        maximum = max(maximum, active)
        await asyncio.sleep(0.01)
        active -= 1
        return JPEG, "image/jpeg"

    requests = [
        ThumbnailRequest(f"video{i:06d}", f"https://img.test/{i}.jpg") for i in range(6)
    ]
    asyncio.run(
        cache_thumbnails(
            requests,
            cache_dir=tmp_path,
            concurrency=2,
            fetcher=fetch,
        )
    )

    assert maximum == 2


def test_thumbnail_cancellation_propagates(tmp_path: Path) -> None:
    token = CancellationToken()
    token.cancel()

    async def fetch(url: str) -> tuple[bytes, str]:
        return JPEG, "image/jpeg"

    with pytest.raises(WorkflowCancelled):
        asyncio.run(
            cache_thumbnails(
                [ThumbnailRequest("abcdefghijk", "https://img.test/a.jpg")],
                cache_dir=tmp_path,
                cancellation=token,
                fetcher=fetch,
            )
        )

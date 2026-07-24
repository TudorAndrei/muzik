import asyncio
from concurrent.futures import ThreadPoolExecutor
import zipfile
from pathlib import Path
from typing import Any, cast

import pytest
from yarl import URL

from muzik.core.bandcamp import (
    BandcampApi,
    DigitalItem,
    DigitalItemDownload,
    ScopedCookie,
    _extract_zip,
    cookie_jar,
    load_cookies,
    safe_download_path,
    run,
    write_netscape_cookies,
    Cache,
)
from rich.progress import Progress


def test_netscape_cookies_preserve_scope_and_origin(tmp_path: Path) -> None:
    cookies_path = tmp_path / "cookies.txt"
    cookies_path.write_text(
        ".bandcamp.com\tTRUE\t/music\tTRUE\t4102444800\tsession\tsecret\n"
    )

    cookies = load_cookies(cookies_path)

    async def check_jar() -> None:
        jar = cookie_jar(cookies)
        assert "session" in jar.filter_cookies(URL("https://bandcamp.com/music/item"))
        assert "session" not in jar.filter_cookies(URL("https://bandcamp.com/other"))
        assert "session" not in jar.filter_cookies(URL("https://example.test/music"))

    asyncio.run(check_jar())

    assert cookies == [
        ScopedCookie(
            domain=".bandcamp.com",
            path="/music",
            secure=True,
            expires=4102444800,
            name="session",
            value="secret",
        )
    ]


def test_cookie_json_and_netscape_round_trip_preserve_scope(tmp_path: Path) -> None:
    json_path = tmp_path / "cookies.json"
    json_path.write_text(
        '[{"Host raw":"https://artist.bandcamp.com",'
        '"Path raw":"/orders","Is secure raw":true,"Expires raw":4102444800,'
        '"Name raw":"session","Content raw":"value"}]'
    )
    assert load_cookies(json_path) == [
        ScopedCookie(
            domain="artist.bandcamp.com",
            path="/orders",
            secure=True,
            expires=4102444800,
            name="session",
            value="value",
        )
    ]

    netscape_path = tmp_path / "cookies.txt"
    write_netscape_cookies(
        [
            {
                "domain": ".bandcamp.com",
                "path": "/music",
                "secure": True,
                "expires": 4102444800,
                "name": "session",
                "value": "value",
            }
        ],
        netscape_path,
    )
    assert load_cookies(netscape_path) == [
        ScopedCookie(
            domain=".bandcamp.com",
            path="/music",
            secure=True,
            expires=4102444800,
            name="session",
            value="value",
        )
    ]


@pytest.mark.parametrize(
    "disposition",
    [
        'attachment; filename="../outside.zip"',
        'attachment; filename="folder/file.zip"',
        'attachment; filename="C:\\\\temp.zip"',
        'attachment; filename=""',
    ],
)
def test_safe_download_path_rejects_unsafe_filenames(
    tmp_path: Path, disposition: str
) -> None:
    with pytest.raises(RuntimeError, match="Unsafe"):
        safe_download_path(tmp_path, disposition)


def test_safe_download_path_keeps_unicode_filename_inside_destination(
    tmp_path: Path,
) -> None:
    assert safe_download_path(tmp_path, 'attachment; filename="Beyoncé.zip"') == (
        tmp_path / "Beyoncé.zip"
    )


def test_extract_zip_rejects_traversal_without_writing_outside(tmp_path: Path) -> None:
    archive = tmp_path / "release.zip"
    outside = tmp_path / "outside.txt"
    with zipfile.ZipFile(archive, "w") as zf:
        zf.writestr("../outside.txt", "bad")

    with pytest.raises(RuntimeError, match="escapes"):
        _extract_zip(archive, tmp_path / "dest")

    assert not outside.exists()


def test_extract_zip_extracts_safe_members(tmp_path: Path) -> None:
    archive = tmp_path / "release.zip"
    dest = tmp_path / "dest"
    with zipfile.ZipFile(archive, "w") as zf:
        zf.writestr("Artist/Track.flac", "audio")

    _extract_zip(archive, dest)

    assert (dest / "Artist" / "Track.flac").read_text() == "audio"
    assert not archive.exists()


def test_interrupted_download_leaves_no_final_or_partial_file(tmp_path: Path) -> None:
    class Content:
        async def iter_chunked(self, size):
            yield b"partial"
            raise OSError("connection lost")

    class Response:
        status = 200
        content_length = 10
        headers = {"Content-Disposition": 'attachment; filename="release.zip"'}
        content = Content()

        def raise_for_status(self):
            return None

    class RequestContext:
        async def __aenter__(self):
            return Response()

        async def __aexit__(self, *args):
            return False

    class Session:
        def get(self, url):
            return RequestContext()

    item = DigitalItem(
        title="Release",
        artist="Artist",
        item_type="album",
        download_type_str="album",
        downloads={"flac": DigitalItemDownload(url="https://example.test/file")},
    )

    async def download() -> None:
        with pytest.raises(OSError, match="connection lost"):
            await BandcampApi(cast(Any, Session())).download_item(
                item, tmp_path, "flac", Progress()
            )

    asyncio.run(download())

    assert not (tmp_path / "release.zip").exists()
    assert not (tmp_path / ".release.zip.part").exists()


def test_cache_reads_once_and_suppresses_concurrent_duplicates(
    tmp_path: Path, monkeypatch
) -> None:
    path = tmp_path / "bandcamp.cache"
    path.write_text("existing| prior\n")
    reads = 0
    original_read_text = Path.read_text

    def counted_read_text(self, *args, **kwargs):
        nonlocal reads
        if self == path:
            reads += 1
        return original_read_text(self, *args, **kwargs)

    monkeypatch.setattr(Path, "read_text", counted_read_text)
    cache = Cache(path)

    with ThreadPoolExecutor(max_workers=8) as pool:
        list(pool.map(lambda _: cache.add_if_missing("same", "release"), range(16)))

    cache.add_if_missing("next", "release")
    assert reads == 1
    assert path.read_text().splitlines() == [
        "existing| prior",
        "same| release",
        "next| release",
    ]


@pytest.mark.parametrize("jobs", [0, -1])
def test_run_rejects_invalid_job_count_before_opening_session(
    tmp_path: Path, jobs: int
) -> None:
    with pytest.raises(ValueError, match="at least 1"):
        run("fan", tmp_path / "cookies.txt", tmp_path / "output", jobs=jobs)

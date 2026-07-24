"""Pure-Python async Bandcamp collection downloader.

Uses aiohttp for HTTP and stamina for retry logic.
Ported from bandsnatch (https://github.com/Ovyerus/bandsnatch).
"""

from __future__ import annotations

import asyncio
import json
import os
import re
import zipfile
from dataclasses import dataclass
from datetime import datetime, timezone
from http.cookies import SimpleCookie
from pathlib import Path
from threading import Lock
from typing import Any, Mapping, Optional, Sequence
from urllib.parse import urlparse

import aiohttp
import stamina
from bs4 import BeautifulSoup
from rich.progress import (
    BarColumn,
    DownloadColumn,
    Progress,
    TextColumn,
    TimeRemainingColumn,
    TransferSpeedColumn,
)
from yarl import URL

from muzik.config import BANDCAMP_CACHE_FILE
from muzik.ui.console import console, err

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

MAX_RETRIES = 5
_PURCHASE_DATE_FORMAT = "%d %b %Y %H:%M:%S %Z"

_FS_REPLACEMENTS: dict[str, str] = {
    ":": "꞉",
    "/": "／",
    "\\": "⧹",
    '"': "＂",
    "*": "⋆",
    "<": "＜",
    ">": "＞",
    "?": "？",
    "|": "∣",
}
_UNSAFE_ENDINGS = (".", " ")


# ---------------------------------------------------------------------------
# Internals
# ---------------------------------------------------------------------------


class _RateLimited(Exception):
    """Raised on HTTP 429 so stamina knows to back off and retry."""


class DigitalItemUnavailable(Exception):
    """A purchased item has no terminally available digital download."""


def _retry():
    """Shared stamina retry context: up to MAX_RETRIES attempts, 10 s constant wait."""
    return stamina.retry_context(
        on=_RateLimited,
        attempts=MAX_RETRIES,
        wait_initial=10.0,
        wait_max=60.0,
        wait_jitter=0.0,
    )


def _make_fs_safe(s: str) -> str:
    for char, replacement in _FS_REPLACEMENTS.items():
        s = s.replace(char, replacement)
    if s.endswith(_UNSAFE_ENDINGS):
        s += "_"
    return s


def _parse_purchase_date(s: str) -> Optional[datetime]:
    try:
        return datetime.strptime(s, _PURCHASE_DATE_FORMAT).replace(tzinfo=timezone.utc)
    except ValueError:
        return None


# ---------------------------------------------------------------------------
# Data classes
# ---------------------------------------------------------------------------


@dataclass
class DownloadInfo:
    url: str
    purchased: Optional[str] = None


@dataclass
class DigitalItemDownload:
    url: str


@dataclass
class DigitalItem:
    title: str
    artist: str
    item_type: str
    download_type_str: str
    downloads: Optional[dict[str, DigitalItemDownload]] = None
    package_release_date: Optional[str] = None
    download_type: Optional[str] = None

    def is_single(self) -> bool:
        return (
            self.download_type == "t"
            or self.download_type_str == "track"
            or self.item_type == "track"
        )

    def release_year(self) -> str:
        if not self.package_release_date:
            return "0000"
        try:
            return str(
                datetime.strptime(
                    self.package_release_date, "%d %b %Y %H:%M:%S %Z"
                ).year
            )
        except ValueError:
            return "0000"

    def destination_path(self, root: Path) -> Path:
        return (
            root
            / _make_fs_safe(self.artist)
            / f"{_make_fs_safe(self.title)} ({self.release_year()})"
        )


@dataclass(frozen=True, slots=True)
class DownloadResult:
    item_id: str
    status: str
    destination: Path | None = None


# ---------------------------------------------------------------------------
# Cookie I/O
# ---------------------------------------------------------------------------


@dataclass(frozen=True, slots=True)
class ScopedCookie:
    domain: str
    path: str
    secure: bool
    expires: int | None
    name: str
    value: str


def _load_cookies_json(path: Path) -> list[ScopedCookie]:
    """Load Firefox Cookie Quick Manager cookies while preserving their scope."""
    result = []
    for c in json.loads(path.read_text()):
        host = c.get("Host raw", "")
        if not host.startswith("http"):
            host = "https://" + host
        domain = urlparse(host).hostname or "bandcamp.com"
        name = c.get("Name raw", "")
        value = c.get("Content raw", "")
        if name:
            result.append(
                ScopedCookie(
                    domain=domain,
                    path=c.get("Path raw") or c.get("path") or "/",
                    secure=bool(c.get("Is secure raw") or c.get("secure")),
                    expires=_cookie_expiry(c.get("Expires raw") or c.get("expires")),
                    name=name,
                    value=value,
                )
            )
    return result


def _load_cookies_netscape(path: Path) -> list[ScopedCookie]:
    """Netscape/Mozilla cookies.txt: tab-separated 7-column format."""
    result = []
    for line in path.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split("\t")
        if len(parts) == 7:
            domain, _, cookie_path, secure, expires, name, value = parts
            result.append(
                ScopedCookie(
                    domain=domain,
                    path=cookie_path or "/",
                    secure=secure.upper() == "TRUE",
                    expires=_cookie_expiry(expires),
                    name=name,
                    value=value,
                )
            )
    return result


def load_cookies(path: Path) -> list[ScopedCookie]:
    """Load cookies from a JSON or Netscape file with origin/path scope."""
    if path.suffix.lower() == ".json":
        return _load_cookies_json(path)
    return _load_cookies_netscape(path)


def write_netscape_cookies(
    raw_cookies: Sequence[Mapping[str, Any]], dest: Path
) -> None:
    """Serialise a Playwright cookie list to Netscape cookies.txt."""
    lines = ["# Netscape HTTP Cookie File\n"]
    for c in raw_cookies:
        domain = c["domain"]
        include_sub = "TRUE" if domain.startswith(".") else "FALSE"
        path = c.get("path", "/")
        secure = "TRUE" if c.get("secure") else "FALSE"
        expires = int(c.get("expires") or 0)
        lines.append(
            f"{domain}\t{include_sub}\t{path}\t{secure}\t{expires}"
            f"\t{c['name']}\t{c['value']}\n"
        )
    dest.write_text("".join(lines))


def _cookie_expiry(value: object) -> int | None:
    if not isinstance(value, (str, int, float)):
        return None
    try:
        expiry = int(float(value))
    except TypeError, ValueError:
        return None
    return expiry or None


def cookie_jar(cookies: Sequence[ScopedCookie]) -> aiohttp.CookieJar:
    """Build an aiohttp jar which retains cookie origin/path restrictions."""
    jar = aiohttp.CookieJar(unsafe=False)
    for cookie in cookies:
        domain = cookie.domain.lstrip(".")
        if not domain:
            continue
        scheme = "https" if cookie.secure else "http"
        response_url = URL.build(scheme=scheme, host=domain, path=cookie.path)
        scoped = SimpleCookie()
        scoped[cookie.name] = cookie.value
        morsel = scoped[cookie.name]
        morsel["path"] = cookie.path
        morsel["domain"] = cookie.domain
        morsel["secure"] = cookie.secure
        if cookie.expires is not None:
            morsel["max-age"] = str(
                max(0, cookie.expires - int(datetime.now().timestamp()))
            )
        jar.update_cookies(scoped, response_url=response_url)
    return jar


# ---------------------------------------------------------------------------
# Cache
# ---------------------------------------------------------------------------


class Cache:
    """Pipe-delimited download cache, compatible with bandcamp-collection-downloader."""

    def __init__(self, path: Path) -> None:
        self._path = path
        self._path.parent.mkdir(parents=True, exist_ok=True)
        self._lock = Lock()
        self._ids = self._read_ids()

    def _read_ids(self) -> set[str]:
        if not self._path.exists():
            return set()
        return {
            line.split("|", 1)[0].strip()
            for line in self._path.read_text().splitlines()
            if line.split("|", 1)[0].strip()
        }

    def content(self) -> set[str]:
        with self._lock:
            return set(self._ids)

    def add(self, item_id: str, description: str) -> None:
        with self._lock:
            with self._path.open("a") as f:
                f.write(f"{item_id}| {description}\n")
            self._ids.add(item_id)

    def add_if_missing(self, item_id: str, description: str) -> None:
        with self._lock:
            if item_id not in self._ids:
                with self._path.open("a") as f:
                    f.write(f"{item_id}| {description}\n")
                self._ids.add(item_id)


# ---------------------------------------------------------------------------
# API
# ---------------------------------------------------------------------------


class BandcampApi:
    def __init__(self, session: aiohttp.ClientSession) -> None:
        self._session = session

    async def _get_text(self, url: str, **kwargs) -> str:
        async for attempt in _retry():
            with attempt:
                async with self._session.get(url, **kwargs) as resp:
                    if resp.status == 429:
                        console.print("  [yellow]Rate limited — retrying…[/yellow]")
                        raise _RateLimited(url)
                    resp.raise_for_status()
                    return await resp.text()
        raise RuntimeError("unreachable")  # stamina raises after exhausting attempts

    async def _post_json(self, url: str, payload: dict) -> dict:
        async for attempt in _retry():
            with attempt:
                async with self._session.post(url, json=payload) as resp:
                    if resp.status == 429:
                        console.print("  [yellow]Rate limited — retrying…[/yellow]")
                        raise _RateLimited(url)
                    resp.raise_for_status()
                    return await resp.json()
        raise RuntimeError("unreachable")

    async def _page_blob(self, url: str) -> dict:
        text = await self._get_text(url)
        soup = BeautifulSoup(text, "html.parser")
        el = soup.find(id="pagedata")
        if not el:
            raise RuntimeError(f"Could not find #pagedata element on {url}")
        blob = el.get("data-blob")
        if not isinstance(blob, str):
            raise RuntimeError(f"Invalid #pagedata blob on {url}")
        return json.loads(blob)

    async def get_download_urls(self, username: str) -> dict[str, DownloadInfo]:
        """Scrape the user's collection page, paginating as needed."""
        data = await self._page_blob(f"https://bandcamp.com/{username}")

        fan_data = data.get("fan_data", {})
        if not fan_data.get("is_own_page"):
            raise RuntimeError(
                f'Failed to scrape collection for "{username}" (is_own_page is false). '
                "Check your cookies or spelling."
            )

        fan_id = str(fan_data["fan_id"])
        item_cache = data.get("item_cache", {}).get("collection", {})
        items_by_key: dict[str, dict] = {
            f"{v['sale_item_type']}{v['sale_item_id']}": v for v in item_cache.values()
        }

        def _enrich(raw: dict[str, str]) -> dict[str, DownloadInfo]:
            return {
                item_id: DownloadInfo(
                    url=url,
                    purchased=items_by_key.get(item_id, {}).get("purchased"),
                )
                for item_id, url in raw.items()
            }

        collection_data = data.get("collection_data", {})
        urls = _enrich(collection_data.get("redownload_urls") or {})

        if (collection_data.get("item_count") or 0) > (
            collection_data.get("batch_size") or 0
        ):
            last_token = collection_data.get("last_token", "")
            more_available = True
            while more_available:
                page = await self._post_json(
                    "https://bandcamp.com/api/fancollection/1/collection_items",
                    {"fan_id": fan_id, "older_than_token": last_token},
                )
                page_items_by_key: dict[str, dict] = {
                    f"{i['sale_item_type']}{i['sale_item_id']}": i
                    for i in page.get("items", [])
                }
                for item_id, url in page.get("redownload_urls", {}).items():
                    urls[item_id] = DownloadInfo(
                        url=url,
                        purchased=page_items_by_key.get(item_id, {}).get("purchased"),
                    )
                more_available = page.get("more_available", False)
                last_token = page.get("last_token", "")

        return urls

    async def get_digital_item(self, url: str) -> DigitalItem:
        """Fetch download metadata for a single purchase page."""
        data = await self._page_blob(url)

        digital_items = data.get("digital_items", [])
        if not digital_items:
            raise DigitalItemUnavailable("No digital item is available.")

        raw = digital_items[0]
        downloads: Optional[dict[str, DigitalItemDownload]] = None
        if raw.get("downloads"):
            downloads = {
                fmt: DigitalItemDownload(url=info["url"])
                for fmt, info in raw["downloads"].items()
            }

        return DigitalItem(
            title=raw.get("title", ""),
            artist=raw.get("artist", ""),
            item_type=raw.get("item_type", ""),
            download_type_str=raw.get("download_type_str", ""),
            download_type=raw.get("download_type"),
            package_release_date=raw.get("package_release_date"),
            downloads=downloads,
        )

    async def download_item(
        self,
        item: DigitalItem,
        dest_dir: Path,
        audio_format: str,
        progress: Progress,
    ) -> None:
        """Stream-download a purchase and extract if it's a ZIP album."""
        if not item.downloads or audio_format not in item.downloads:
            available = list(item.downloads or {})
            raise RuntimeError(
                f"{item.artist} - {item.title}: format {audio_format!r} not available "
                f"(got: {available})"
            )

        download_url = item.downloads[audio_format].url

        async for attempt in _retry():
            with attempt:
                async with self._session.get(download_url) as resp:
                    if resp.status == 429:
                        raise _RateLimited(download_url)
                    resp.raise_for_status()

                    dest_dir.mkdir(parents=True, exist_ok=True)
                    dest_path = safe_download_path(
                        dest_dir, resp.headers.get("Content-Disposition", "")
                    )
                    label = f"{item.artist} - {item.title}"
                    task = progress.add_task(label, total=resp.content_length)

                    partial = dest_path.with_name(f".{dest_path.name}.part")
                    try:
                        with partial.open("wb") as f:
                            async for chunk in resp.content.iter_chunked(8192):
                                f.write(chunk)
                                progress.advance(task, len(chunk))
                        os.replace(partial, dest_path)
                    except Exception:
                        partial.unlink(missing_ok=True)
                        raise
                    finally:
                        progress.remove_task(task)

        if not item.is_single():
            await asyncio.to_thread(_extract_zip, dest_path, dest_dir)


def safe_download_path(dest_dir: Path, disposition: str) -> Path:
    """Return a safe path for a Content-Disposition filename in *dest_dir*."""
    match = re.search(
        r"filename\*?=(?:UTF-8''|)?(?:\"([^\"]+)\"|'([^']+)'|([^;]+))",
        disposition,
        re.I,
    )
    filename = next((part for part in match.groups() if part), "") if match else ""
    filename = filename.strip().strip('"').strip("'")
    if (
        not filename
        or any(char in filename for char in ("/", "\\", "\x00"))
        or Path(filename).is_absolute()
        or re.match(r"^[A-Za-z]:", filename)
        or filename in {".", ".."}
    ):
        raise RuntimeError("Unsafe or missing Content-Disposition filename.")
    destination = (dest_dir.resolve() / filename).resolve()
    try:
        destination.relative_to(dest_dir.resolve())
    except ValueError as exc:
        raise RuntimeError(
            "Download filename escapes its destination directory."
        ) from exc
    return destination


def _extract_zip(path: Path, dest_dir: Path) -> None:
    with zipfile.ZipFile(path) as zf:
        root = dest_dir.resolve()
        for member in zf.infolist():
            target = (root / member.filename).resolve()
            try:
                target.relative_to(root)
            except ValueError as exc:
                raise RuntimeError(
                    "ZIP member escapes its destination directory."
                ) from exc
        zf.extractall(dest_dir)
    path.unlink()


# ---------------------------------------------------------------------------
# Orchestration
# ---------------------------------------------------------------------------


async def _run_async(
    username: str,
    cookies_path: Path,
    output: Path,
    audio_format: str,
    jobs: int,
    force: bool,
    dry_run: bool,
    after: Optional[datetime],
    limit: Optional[int],
) -> list[Path]:
    if jobs < 1:
        raise ValueError("jobs must be at least 1")
    output.mkdir(parents=True, exist_ok=True)

    cookies = load_cookies(cookies_path)
    cache = Cache(BANDCAMP_CACHE_FILE)

    headers = {
        "User-Agent": (
            "Mozilla/5.0 (X11; Linux x86_64; rv:120.0) Gecko/20100101 Firefox/120.0"
        )
    }

    async with aiohttp.ClientSession(
        cookie_jar=cookie_jar(cookies),
        headers=headers,
        connector=aiohttp.TCPConnector(limit=jobs),
    ) as session:
        api = BandcampApi(session)

        console.print(f"  [dim]Fetching collection for {username}…[/dim]")
        all_urls = await api.get_download_urls(username)

        cached_ids = set() if force else cache.content()
        items = [
            (item_id, info)
            for item_id, info in all_urls.items()
            if item_id not in cached_ids
        ]
        if limit is not None:
            items = items[:limit]

        if dry_run:
            console.print(f"  [dim]Would download {len(items)} release(s)[/dim]")
            for item_id, _ in items:
                console.print(f"  [dim]{item_id}[/dim]")
            return []

        console.print(f"  Downloading [bold]{len(items)}[/bold] release(s)")

        progress = Progress(
            TextColumn("[cyan]{task.description}[/cyan]"),
            BarColumn(),
            DownloadColumn(),
            TransferSpeedColumn(),
            TimeRemainingColumn(),
            console=console,
        )
        sem = asyncio.Semaphore(jobs)

        async def _download_one(item_id: str, info: DownloadInfo) -> DownloadResult:
            async with sem:
                if after and info.purchased:
                    purchased_dt = _parse_purchase_date(info.purchased)
                    if purchased_dt and purchased_dt < after:
                        cache.add_if_missing(item_id, "Skipped (--after filter)")
                        return DownloadResult(item_id, "skipped")

                try:
                    item = await api.get_digital_item(info.url)
                except DigitalItemUnavailable:
                    cache.add(item_id, "UNKNOWN")
                    return DownloadResult(item_id, "unavailable")
                except Exception as exc:
                    err(f"  [red]Failed to get item info for {item_id}: {exc}[/red]")
                    return DownloadResult(item_id, "failed")

                if not item.downloads:
                    console.print(
                        f"  [yellow]Skipping {item_id} — no downloads available[/yellow]"
                    )
                    cache.add(item_id, "No downloads")
                    return DownloadResult(item_id, "unavailable")

                dest = item.destination_path(output)
                try:
                    await api.download_item(item, dest, audio_format, progress)
                except Exception as exc:
                    err(f"  [red]Failed {item.artist} - {item.title}: {exc}[/red]")
                    return DownloadResult(item_id, "failed")

                cache.add_if_missing(
                    item_id,
                    f"{item.title} ({item.release_year()}) by {item.artist}",
                )
                console.print(f"  [green]✓[/green] {item.artist} - {item.title}")
                return DownloadResult(item_id, "downloaded", dest)

        with progress:
            results = await asyncio.gather(
                *[_download_one(item_id, info) for item_id, info in items]
            )

    console.print("[green]Finished![/green]")
    return [result.destination for result in results if result.destination is not None]


def run(
    username: str,
    cookies_path: Path,
    output: Path,
    audio_format: str = "flac",
    jobs: int = 4,
    force: bool = False,
    dry_run: bool = False,
    after: Optional[datetime] = None,
    limit: Optional[int] = None,
) -> list[Path]:
    """Synchronous entry point — runs the async downloader via asyncio.run()."""
    if jobs < 1:
        raise ValueError("jobs must be at least 1")
    return asyncio.run(
        _run_async(
            username,
            cookies_path,
            output,
            audio_format,
            jobs,
            force,
            dry_run,
            after,
            limit,
        )
    )

"""Pure-Python async Bandcamp collection downloader.

Uses aiohttp for HTTP and stamina for retry logic.
Ported from bandsnatch (https://github.com/Ovyerus/bandsnatch).
"""

from __future__ import annotations

import asyncio
import json
import zipfile
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from threading import Lock
from typing import Optional
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


def _retry():
    """Shared stamina retry context: up to MAX_RETRIES attempts, 10 s constant wait."""
    return stamina.retry_context(
        on=_RateLimited,
        attempts=MAX_RETRIES,
        wait_initial=10.0,
        wait_max=60.0,
        wait_jitter=0.0,
        wait_exp=1.0,
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
                datetime.strptime(self.package_release_date, "%d %b %Y %H:%M:%S %Z").year
            )
        except ValueError:
            return "0000"

    def destination_path(self, root: Path) -> Path:
        return (
            root
            / _make_fs_safe(self.artist)
            / f"{_make_fs_safe(self.title)} ({self.release_year()})"
        )


# ---------------------------------------------------------------------------
# Cookie I/O
# ---------------------------------------------------------------------------


def _load_cookies_json(path: Path) -> list[tuple[str, str, str]]:
    """Firefox Cookie Quick Manager format: [{Host raw, Name raw, Content raw}]."""
    result = []
    for c in json.loads(path.read_text()):
        host = c.get("Host raw", "")
        if not host.startswith("http"):
            host = "https://" + host
        domain = urlparse(host).hostname or "bandcamp.com"
        name = c.get("Name raw", "")
        value = c.get("Content raw", "")
        if name:
            result.append((domain, name, value))
    return result


def _load_cookies_netscape(path: Path) -> list[tuple[str, str, str]]:
    """Netscape/Mozilla cookies.txt: tab-separated 7-column format."""
    result = []
    for line in path.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split("\t")
        if len(parts) == 7:
            domain, _, _, _, _, name, value = parts
            result.append((domain, name, value))
    return result


def load_cookies(path: Path) -> list[tuple[str, str, str]]:
    """Load cookies from a JSON or Netscape file. Returns [(domain, name, value)]."""
    if path.suffix.lower() == ".json":
        return _load_cookies_json(path)
    return _load_cookies_netscape(path)


def write_netscape_cookies(raw_cookies: list[dict], dest: Path) -> None:
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


# ---------------------------------------------------------------------------
# Cache
# ---------------------------------------------------------------------------


class Cache:
    """Pipe-delimited download cache, compatible with bandcamp-collection-downloader."""

    def __init__(self, path: Path) -> None:
        self._path = path
        self._path.parent.mkdir(parents=True, exist_ok=True)
        self._lock = Lock()

    def content(self) -> set[str]:
        if not self._path.exists():
            return set()
        ids: set[str] = set()
        for line in self._path.read_text().splitlines():
            item_id = line.split("|", 1)[0].strip()
            if item_id:
                ids.add(item_id)
        return ids

    def add(self, item_id: str, description: str) -> None:
        with self._lock:
            with self._path.open("a") as f:
                f.write(f"{item_id}| {description}\n")

    def add_if_missing(self, item_id: str, description: str) -> None:
        with self._lock:
            if item_id not in self.content():
                with self._path.open("a") as f:
                    f.write(f"{item_id}| {description}\n")


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
        return json.loads(el["data-blob"])  # type: ignore[index]

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

        if (collection_data.get("item_count") or 0) > (collection_data.get("batch_size") or 0):
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

    async def get_digital_item(self, url: str) -> Optional[DigitalItem]:
        """Fetch download metadata for a single purchase page."""
        try:
            data = await self._page_blob(url)
        except Exception as exc:
            err(f"  [red]Failed to get item info for {url}: {exc}[/red]")
            return None

        digital_items = data.get("digital_items", [])
        if not digital_items:
            return None

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

                    disposition = resp.headers.get("Content-Disposition", "")
                    filename: Optional[str] = None
                    for part in disposition.split(";"):
                        part = part.strip()
                        if part.lower().startswith("filename="):
                            filename = part[9:].strip().strip('"').strip("'")
                            break
                    if not filename:
                        raise RuntimeError(
                            f"No Content-Disposition filename for "
                            f"{item.artist} - {item.title}"
                        )

                    dest_dir.mkdir(parents=True, exist_ok=True)
                    dest_path = dest_dir / filename
                    label = f"{item.artist} - {item.title}"
                    task = progress.add_task(label, total=resp.content_length)

                    with dest_path.open("wb") as f:
                        async for chunk in resp.content.iter_chunked(8192):
                            f.write(chunk)
                            progress.advance(task, len(chunk))

                    progress.remove_task(task)

        if not item.is_single():
            await asyncio.to_thread(_extract_zip, dest_path, dest_dir)


def _extract_zip(path: Path, dest_dir: Path) -> None:
    with zipfile.ZipFile(path) as zf:
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
) -> None:
    output.mkdir(parents=True, exist_ok=True)

    cookies = {name: value for _, name, value in load_cookies(cookies_path)}
    cache = Cache(BANDCAMP_CACHE_FILE)

    headers = {
        "User-Agent": (
            "Mozilla/5.0 (X11; Linux x86_64; rv:120.0) Gecko/20100101 Firefox/120.0"
        )
    }

    async with aiohttp.ClientSession(
        cookies=cookies,
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
            return

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

        async def _download_one(item_id: str, info: DownloadInfo) -> None:
            async with sem:
                if after and info.purchased:
                    purchased_dt = _parse_purchase_date(info.purchased)
                    if purchased_dt and purchased_dt < after:
                        cache.add_if_missing(item_id, "Skipped (--after filter)")
                        return

                item = await api.get_digital_item(info.url)
                if item is None:
                    cache.add(item_id, "UNKNOWN")
                    return

                if not item.downloads:
                    console.print(
                        f"  [yellow]Skipping {item_id} — no downloads available[/yellow]"
                    )
                    cache.add(item_id, "No downloads")
                    return

                dest = item.destination_path(output)
                try:
                    await api.download_item(item, dest, audio_format, progress)
                except Exception as exc:
                    err(f"  [red]Failed {item.artist} - {item.title}: {exc}[/red]")
                    return

                cache.add_if_missing(
                    item_id,
                    f"{item.title} ({item.release_year()}) by {item.artist}",
                )
                console.print(f"  [green]✓[/green] {item.artist} - {item.title}")

        with progress:
            await asyncio.gather(
                *[_download_one(item_id, info) for item_id, info in items]
            )

    console.print("[green]Finished![/green]")


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
) -> None:
    """Synchronous entry point — runs the async downloader via asyncio.run()."""
    asyncio.run(
        _run_async(username, cookies_path, output, audio_format, jobs, force, dry_run, after, limit)
    )

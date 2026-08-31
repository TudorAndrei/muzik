"""YouTube source support backed by yt-dlp."""

from __future__ import annotations

import json
import os
import re
import shutil
from dataclasses import dataclass
from pathlib import Path
from typing import Optional

from muzik.config import AUDIO_EXTENSIONS, CACHE_DIR, YTDLP_OUTPUT_TEMPLATE
from muzik.core import cache as cache_mod
from muzik.core.metadata import write_muzik_metadata
from muzik.core.runner import run_silent, run_streaming
from muzik.core.workflow.cancellation import CancellationToken
from muzik.core.sources.base import (
    Candidate,
    DownloadRequest,
    DownloadResult,
    ResolvedPlaylist,
    ResolvedRelease,
    ResolvedTrack,
)


YOUTUBE_ID_RE = re.compile(r"(?:v=|youtu\.be/|/v/|/embed/)([A-Za-z0-9_-]{11})")
YOUTUBE_PLAYLIST_RE = re.compile(r"[?&]list=([A-Za-z0-9_-]+)")
_YOUTUBE_VIDEO_ID_RE = re.compile(r"^[A-Za-z0-9_-]{11}$")


class PlaylistLookupError(RuntimeError):
    """Raised when yt-dlp cannot return a usable playlist document."""


@dataclass(frozen=True, slots=True)
class YouTubePlaylistItem:
    """One item returned by a flat YouTube playlist lookup."""

    position: int
    title: str
    video_id: str | None
    video_url: str | None
    thumbnail_url: str | None


def js_runtime_args() -> list[str]:
    """Enable a JavaScript runtime for yt-dlp if one is on PATH.

    Recent YouTube extraction needs a JS runtime to solve player challenges;
    without one yt-dlp finds no formats and downloads nothing. Muzik enables an
    installed Node.js or Bun runtime explicitly.
    """
    for runtime in ("node", "bun"):
        if shutil.which(runtime):
            return ["--js-runtimes", runtime]
    return []


def cookie_args() -> list[str]:
    """Return yt-dlp cookie flags from the environment, if configured.

    YouTube may demand a signed-in session ("confirm you're not a bot"),
    especially after heavy use. Set ``MUZIK_YTDLP_COOKIES_FROM_BROWSER`` to a
    browser name (e.g. ``chrome``, ``firefox``, ``safari``) or
    ``MUZIK_YTDLP_COOKIES`` to a cookies.txt path to pass one through.
    """
    browser = os.environ.get("MUZIK_YTDLP_COOKIES_FROM_BROWSER", "").strip()
    if browser:
        return ["--cookies-from-browser", browser]
    cookiefile = os.environ.get("MUZIK_YTDLP_COOKIES", "").strip()
    if cookiefile:
        return ["--cookies", cookiefile]
    return []


def youtube_id(url: str) -> Optional[str]:
    """Extract the 11-char YouTube video ID from a URL."""
    match = YOUTUBE_ID_RE.search(url)
    return match.group(1) if match else None


def playlist_id(url: str) -> Optional[str]:
    """Extract a YouTube playlist ID from a URL."""
    match = YOUTUBE_PLAYLIST_RE.search(url)
    return match.group(1) if match else None


def video_id_from_path(path: Path) -> Optional[str]:
    """Extract a YouTube ID from filenames like `Title [ID].flac`."""
    match = re.search(r"\[([A-Za-z0-9_-]{11})\]", path.stem)
    return match.group(1) if match else None


def build_download_command(
    url: str,
    *,
    format: str = "bestaudio",  # noqa: A002
    quality: str = "0",
    no_chapters: bool = False,
    archive_file: Optional[Path] = None,
    force: bool = False,
) -> list[str]:
    """Build the yt-dlp audio download command.

    When *force* is set, add ``--force-overwrites`` so an already-downloaded
    file is fetched again instead of skipped. The caller also drops the download
    archive under force, so the skip cannot come from either side.
    """
    cmd = [
        "yt-dlp",
        *cookie_args(),
        *js_runtime_args(),
        "--format",
        format,
        "--extract-audio",
        "--audio-quality",
        quality,
        "--embed-metadata",
        "--add-metadata",
        # Save the video thumbnail as a JPEG sidecar for album-cover fallback.
        "--write-thumbnail",
        "--convert-thumbnails",
        "jpg",
        "--output",
        YTDLP_OUTPUT_TEMPLATE,
    ]
    if not no_chapters:
        cmd += ["--write-info-json", "--embed-chapters"]
    if force:
        cmd.append("--force-overwrites")
    if archive_file:
        cmd += ["--download-archive", str(archive_file)]
    cmd.append(url)
    return cmd


def audio_files_in(directory: Path) -> list[Path]:
    """Return supported audio files directly under *directory*."""
    if not directory.exists():
        return []
    return sorted(
        file
        for file in directory.iterdir()
        if file.is_file() and file.suffix.lower() in AUDIO_EXTENSIONS
    )


def new_audio_files(before: set[Path], after: set[Path]) -> list[Path]:
    """Return audio files present in *after* but not *before*."""
    return sorted(
        file
        for file in (after - before)
        if file.is_file() and file.suffix.lower() in AUDIO_EXTENSIONS
    )


def find_audio_by_id(directory: Path, yt_id: str) -> list[Path]:
    """Return audio files whose filename contains the YouTube ID."""
    if not directory.exists():
        return []
    return sorted(
        file
        for file in directory.iterdir()
        if file.is_file()
        and file.suffix.lower() in AUDIO_EXTENSIONS
        and f"[{yt_id}]" in file.name
    )


def get_playlist_video_ids(url: str) -> list[str]:
    """Return ordered video IDs in a YouTube playlist via yt-dlp."""
    result = run_silent(
        [
            "yt-dlp",
            *cookie_args(),
            *js_runtime_args(),
            "--flat-playlist",
            "--print",
            "%(id)s",
            url,
        ]
    )
    if result.returncode != 0:
        return []
    return [line.strip() for line in result.stdout.splitlines() if line.strip()]


def get_playlist_items(url: str) -> list[YouTubePlaylistItem]:
    """Return ordered item metadata from one flat YouTube playlist lookup."""
    result = run_silent(
        [
            "yt-dlp",
            *cookie_args(),
            *js_runtime_args(),
            "--flat-playlist",
            "--dump-single-json",
            url,
        ]
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or f"yt-dlp exited with code {result.returncode}"
        raise PlaylistLookupError(f"Unable to read YouTube playlist: {detail}")
    try:
        payload = json.loads(result.stdout)
    except json.JSONDecodeError as exc:
        raise PlaylistLookupError("yt-dlp returned invalid playlist JSON.") from exc
    if not isinstance(payload, dict) or not isinstance(payload.get("entries"), list):
        raise PlaylistLookupError("yt-dlp returned no playlist entries.")

    items: list[YouTubePlaylistItem] = []
    for fallback_position, raw in enumerate(payload["entries"], start=1):
        if not isinstance(raw, dict):
            items.append(
                YouTubePlaylistItem(
                    position=fallback_position,
                    title="Unavailable video",
                    video_id=None,
                    video_url=None,
                    thumbnail_url=None,
                )
            )
            continue
        position = _positive_int(raw.get("playlist_index"), fallback_position)
        raw_id = raw.get("id")
        video_id = (
            str(raw_id)
            if raw_id is not None and _YOUTUBE_VIDEO_ID_RE.fullmatch(str(raw_id))
            else None
        )
        title = str(raw.get("title") or "").strip() or "Unavailable video"
        video_url = _playlist_item_url(raw, video_id)
        items.append(
            YouTubePlaylistItem(
                position=position,
                title=title,
                video_id=video_id,
                video_url=video_url,
                thumbnail_url=_playlist_thumbnail_url(raw),
            )
        )
    return items


def _positive_int(value: object, fallback: int) -> int:
    if not isinstance(value, (int, str)) or isinstance(value, bool):
        return fallback
    try:
        parsed = int(value)
    except TypeError, ValueError:
        return fallback
    return parsed if parsed > 0 else fallback


def _playlist_item_url(raw: dict, video_id: str | None) -> str | None:
    webpage_url = raw.get("webpage_url")
    if isinstance(webpage_url, str) and webpage_url.startswith(("http://", "https://")):
        return webpage_url
    if video_id:
        return f"https://www.youtube.com/watch?v={video_id}"
    return None


def _playlist_thumbnail_url(raw: dict) -> str | None:
    thumbnail = raw.get("thumbnail")
    if isinstance(thumbnail, str) and thumbnail.startswith(("http://", "https://")):
        return thumbnail
    thumbnails = raw.get("thumbnails")
    if not isinstance(thumbnails, list):
        return None
    for candidate in reversed(thumbnails):
        if not isinstance(candidate, dict):
            continue
        candidate_url = candidate.get("url")
        if isinstance(candidate_url, str) and candidate_url.startswith(
            ("http://", "https://")
        ):
            return candidate_url
    return None


def prepopulate_archive(archive_file: Path) -> None:
    """Seed a yt-dlp archive from legacy `yt_<id>` cache entries."""
    existing: set[str] = set()
    if archive_file.exists():
        for line in archive_file.read_text().splitlines():
            parts = line.strip().split()
            if len(parts) >= 2:
                existing.add(parts[1])

    new_lines: list[str] = []
    for path in CACHE_DIR.glob("yt_*.txt"):
        vid_id = path.stem[3:]
        if re.fullmatch(r"[A-Za-z0-9_-]{11}", vid_id) and vid_id not in existing:
            new_lines.append(f"youtube {vid_id}\n")

    if new_lines:
        CACHE_DIR.mkdir(parents=True, exist_ok=True)
        with archive_file.open("a") as fh:
            fh.writelines(new_lines)


def dump_json(url: str, *, flat_playlist: bool = False) -> Optional[dict]:
    """Return `yt-dlp --dump-json` metadata for *url*."""
    cmd = ["yt-dlp", *cookie_args(), *js_runtime_args(), "--dump-json"]
    if flat_playlist:
        cmd.append("--flat-playlist")
    cmd.append(url)
    result = run_silent(cmd)
    if result.returncode != 0:
        return None
    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError:
        return None


class YouTubeSource:
    """YouTube source implementation backed by yt-dlp."""

    name = "youtube"

    def resolve(
        self, request: DownloadRequest
    ) -> ResolvedRelease | ResolvedPlaylist | ResolvedTrack:
        pl_id = playlist_id(request.raw)
        if pl_id:
            video_ids = get_playlist_video_ids(request.raw)
            return ResolvedPlaylist(
                title=pl_id,
                entries=[
                    ResolvedTrack(
                        title=vid_id,
                        source="youtube",
                        source_id=vid_id,
                        source_url=f"https://www.youtube.com/watch?v={vid_id}",
                    )
                    for vid_id in video_ids
                ],
                source="youtube",
                source_id=pl_id,
                source_url=request.raw,
            )

        data = dump_json(request.raw) or {}
        vid_id = data.get("id") or youtube_id(request.raw) or request.raw
        title = data.get("title") or str(vid_id)
        return ResolvedTrack(
            title=title,
            artist=data.get("artist") or data.get("uploader"),
            album=data.get("album"),
            year=(data.get("upload_date") or "")[:4] or None,
            duration=data.get("duration"),
            source="youtube",
            source_id=str(vid_id),
            source_url=request.raw,
            source_metadata=data,
        )

    def search(self, resolved: ResolvedRelease | ResolvedTrack) -> list[Candidate]:
        source_id = resolved.source_id or resolved.source_url or resolved.title
        return [
            Candidate(
                source=self.name,
                source_id=source_id,
                title=resolved.title,
                path=resolved.source_url,
                metadata=resolved.to_dict(),
            )
        ]

    def download(
        self,
        candidate: Candidate,
        output: Path,
        *,
        format: str = "bestaudio",  # noqa: A002
        quality: str = "0",
        no_chapters: bool = False,
        archive_file: Optional[Path] = None,
        force: bool = False,
        cancellation: CancellationToken | None = None,
    ) -> DownloadResult:
        output.mkdir(parents=True, exist_ok=True)
        before = set(output.glob("*")) if output.exists() else set()
        url = candidate.path or candidate.source_id
        command = build_download_command(
            url,
            format=format,
            quality=quality,
            no_chapters=no_chapters,
            archive_file=archive_file,
            force=force,
        )
        if cancellation is None:
            rc = run_streaming(command, cwd=output, label="yt-dlp")
        else:
            rc = run_streaming(
                command,
                cwd=output,
                label="yt-dlp",
                cancellation=cancellation,
            )
        if rc != 0:
            raise RuntimeError(f"yt-dlp exited with code {rc}")

        after = set(output.glob("*")) if output.exists() else set()
        files = new_audio_files(before, after)
        yt_id = youtube_id(url) or candidate.source_id
        if not files and yt_id:
            files = find_audio_by_id(output, yt_id)

        metadata_path = None
        if files:
            metadata_path = write_muzik_metadata(
                files[0],
                {
                    "source": self.name,
                    "source_id": candidate.source_id,
                    "requested": url,
                    "resolved": candidate.metadata,
                    "candidate": candidate.to_dict(),
                },
            )
            cache_mod.set(
                cache_mod.download_cache_key(self.name, candidate.source_id),
                str(files[0].resolve()),
            )
        return DownloadResult(
            source=self.name,
            source_id=candidate.source_id,
            files=files,
            root=output,
            metadata_path=metadata_path,
            metadata=candidate.metadata,
        )

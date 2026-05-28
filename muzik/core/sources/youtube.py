"""YouTube source support backed by yt-dlp."""

from __future__ import annotations

import json
import re
from pathlib import Path
from typing import Optional

from muzik.config import AUDIO_EXTENSIONS, CACHE_DIR, YTDLP_OUTPUT_TEMPLATE
from muzik.core import cache as cache_mod
from muzik.core.metadata import write_muzik_metadata
from muzik.core.runner import run_silent, run_streaming
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
) -> list[str]:
    """Build the yt-dlp audio download command."""
    cmd = [
        "yt-dlp",
        "--format",
        format,
        "--extract-audio",
        "--audio-quality",
        quality,
        "--embed-metadata",
        "--add-metadata",
        "--output",
        YTDLP_OUTPUT_TEMPLATE,
    ]
    if not no_chapters:
        cmd += ["--write-info-json", "--embed-chapters"]
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
    result = run_silent(["yt-dlp", "--flat-playlist", "--print", "%(id)s", url])
    if result.returncode != 0:
        return []
    return [line.strip() for line in result.stdout.splitlines() if line.strip()]


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
    cmd = ["yt-dlp", "--dump-json"]
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
    ) -> DownloadResult:
        output.mkdir(parents=True, exist_ok=True)
        before = set(output.glob("*")) if output.exists() else set()
        url = candidate.path or candidate.source_id
        rc = run_streaming(
            build_download_command(
                url,
                format=format,
                quality=quality,
                no_chapters=no_chapters,
                archive_file=archive_file,
            ),
            cwd=output,
            label="yt-dlp",
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

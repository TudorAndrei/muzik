"""Chapter parsing, serialization, and discovery.

Supported formats
-----------------
.chapters.txt sidecar  — one line per chapter, ``HH:MM:SS Title`` or ``MM:SS Title``
.info.json sidecar     — yt-dlp JSON with a ``chapters`` array
"""

import json
import re
import unicodedata
from dataclasses import dataclass
from pathlib import Path
from typing import Optional


# ---------------------------------------------------------------------------
# Data model
# ---------------------------------------------------------------------------


@dataclass
class Chapter:
    index: int  # 1-based track number
    start: int  # seconds from start of file
    end: Optional[int]  # seconds; None for the last track
    title: str

    @property
    def start_ts(self) -> str:
        return _secs_to_ts(self.start)

    @property
    def end_ts(self) -> Optional[str]:
        return _secs_to_ts(self.end) if self.end is not None else None

    @property
    def duration(self) -> Optional[int]:
        return (self.end - self.start) if self.end is not None else None

    @property
    def duration_str(self) -> str:
        d = self.duration
        return _secs_to_ts(d) if d is not None else "?"


# ---------------------------------------------------------------------------
# Timestamp helpers
# ---------------------------------------------------------------------------


def _ts_to_secs(ts: str) -> int:
    """Convert ``HH:MM:SS`` or ``MM:SS`` to integer seconds."""
    parts = ts.strip().split(":")
    if len(parts) == 3:
        h, m, s = parts
        return int(h) * 3600 + int(m) * 60 + int(s)
    if len(parts) == 2:
        m, s = parts
        return int(m) * 60 + int(s)
    raise ValueError(f"Cannot parse timestamp: {ts!r}")


def _secs_to_ts(secs: int) -> str:
    """Convert integer seconds to ``HH:MM:SS``."""
    h = secs // 3600
    m = (secs % 3600) // 60
    s = secs % 60
    return f"{h:02d}:{m:02d}:{s:02d}"


# ---------------------------------------------------------------------------
# Parsers
# ---------------------------------------------------------------------------

_CHAPTER_RE = re.compile(r"^(\d+:\d{2}(?::\d{2})?)\s+(.+)$")


def parse_chapters_txt(path: Path) -> list[Chapter]:
    """Parse a ``.chapters.txt`` file.

    Each valid line is ``HH:MM:SS Title`` or ``MM:SS Title``.
    Returns chapters with end times derived from successive start times.
    """
    raw: list[tuple[int, str]] = []
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        line = line.strip()
        if not line:
            continue
        m = _CHAPTER_RE.match(line)
        if m:
            raw.append((_ts_to_secs(m.group(1)), m.group(2).strip()))

    chapters: list[Chapter] = []
    for idx, (start, title) in enumerate(raw):
        end = raw[idx + 1][0] if idx + 1 < len(raw) else None
        chapters.append(Chapter(index=idx + 1, start=start, end=end, title=title))
    return chapters


def parse_chapters_json(path: Path) -> list[Chapter]:
    """Parse chapters from a yt-dlp ``.info.json`` file.

    Returns an empty list when the file has no chapters.
    """
    data = json.loads(path.read_text(encoding="utf-8", errors="replace"))
    raw = data.get("chapters") or []
    if not raw:
        return []

    chapters: list[Chapter] = []
    for idx, ch in enumerate(raw):
        start = int(float(ch.get("start_time", 0)))
        end_raw = ch.get("end_time")
        end = int(float(end_raw)) if end_raw is not None else None
        title = ch.get("title") or f"Track {idx + 1}"
        chapters.append(Chapter(index=idx + 1, start=start, end=end, title=title))

    # Fill missing end times from the next chapter's start
    for i in range(len(chapters) - 1):
        if chapters[i].end is None:
            chapters[i] = Chapter(
                index=chapters[i].index,
                start=chapters[i].start,
                end=chapters[i + 1].start,
                title=chapters[i].title,
            )
    return chapters


# ---------------------------------------------------------------------------
# Discovery
# ---------------------------------------------------------------------------


def find_chapters(audio_path: Path) -> list[Chapter]:
    """Locate and parse chapters for *audio_path*.

    Search order:
    1. ``<stem>.chapters.txt`` sidecar
    2. ``<stem>.info.json`` sidecar (yt-dlp)
    Returns an empty list when nothing is found.
    """
    base = audio_path.with_suffix("")

    txt = base.with_suffix(".chapters.txt")
    if txt.exists() and txt.stat().st_size > 0:
        return parse_chapters_txt(txt)

    jsn = base.with_suffix(".info.json")
    if jsn.exists():
        chapters = parse_chapters_json(jsn)
        if chapters:
            return chapters

    return []


# ---------------------------------------------------------------------------
# Serialization (for editor round-trip)
# ---------------------------------------------------------------------------


def serialize_chapters(chapters: list[Chapter]) -> str:
    """Format chapters as a ``.chapters.txt`` string for editing."""
    return "\n".join(f"{ch.start_ts} {ch.title}" for ch in chapters) + "\n"


# ---------------------------------------------------------------------------
# Filename helper shared across commands
# ---------------------------------------------------------------------------


def safe_filename(name: str) -> str:
    """Return an ASCII-safe filename-friendly slug."""
    # Transliterate to ASCII
    name = unicodedata.normalize("NFKD", name)
    name = name.encode("ascii", "ignore").decode("ascii")
    # Strip filesystem-unsafe characters
    name = re.sub(r'[<>:"/\\|?*\x00-\x1f]', "", name)
    # Collapse whitespace to hyphens
    name = re.sub(r"\s+", "-", name.strip())
    name = re.sub(r"-+", "-", name).strip("-")
    return name.lower() or "unknown"

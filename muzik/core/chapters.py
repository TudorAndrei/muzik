"""Chapter parsing, serialization, and discovery.

Supported formats
-----------------
.chapters.txt sidecar  — one line per chapter, ``HH:MM:SS Title`` or ``MM:SS Title``
.info.json sidecar     — yt-dlp JSON with a ``chapters`` array
.cue sidecar           — common album cue sheet with ``TRACK``/``INDEX 01`` entries
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
_CUE_TRACK_RE = re.compile(r"^\s*TRACK\s+(\d+)\s+\S+", re.IGNORECASE)
_CUE_TITLE_RE = re.compile(r'^\s*TITLE\s+"?(.*?)"?\s*$', re.IGNORECASE)
_CUE_INDEX_RE = re.compile(r"^\s*INDEX\s+01\s+(\d{2}):(\d{2}):(\d{2})", re.IGNORECASE)


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


def _cue_ts_to_secs(minutes: str, seconds: str, frames: str) -> int:
    """Convert CUE MM:SS:FF timestamps to whole seconds."""
    total = int(minutes) * 60 + int(seconds)
    if int(frames) >= 38:
        total += 1
    return total


def parse_cue(path: Path) -> list[Chapter]:
    """Parse a CUE sheet into chapters.

    The parser intentionally uses only track ``TITLE`` and ``INDEX 01`` data,
    which is enough to split the common Soulseek case of one album audio file
    plus a same-folder cue sheet.
    """
    raw: list[dict[str, object]] = []
    current: dict[str, object] | None = None

    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        track_match = _CUE_TRACK_RE.match(line)
        if track_match:
            current = {
                "index": int(track_match.group(1)),
                "title": f"Track {int(track_match.group(1))}",
            }
            raw.append(current)
            continue

        if current is None:
            continue

        title_match = _CUE_TITLE_RE.match(line)
        if title_match:
            current["title"] = title_match.group(1).strip() or current["title"]
            continue

        index_match = _CUE_INDEX_RE.match(line)
        if index_match:
            current["start"] = _cue_ts_to_secs(*index_match.groups())

    entries: list[tuple[int, int, str]] = []
    for entry in raw:
        index = entry.get("index")
        start = entry.get("start")
        title = entry.get("title")
        if isinstance(index, int) and isinstance(start, int):
            entries.append((index, start, str(title)))
    entries.sort(key=lambda entry: entry[0])

    chapters: list[Chapter] = []
    for idx, (index, start, title) in enumerate(entries):
        end = entries[idx + 1][1] if idx + 1 < len(entries) else None
        chapters.append(
            Chapter(
                index=index,
                start=start,
                end=end,
                title=title,
            )
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
    3. ``<stem>.cue`` sidecar, then a single ``*.cue`` in the same directory
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

    cue = base.with_suffix(".cue")
    cue_candidates = [cue] if cue.exists() else []
    if not cue_candidates and audio_path.parent.exists():
        cue_candidates = sorted(audio_path.parent.glob("*.cue"))
        if len(cue_candidates) != 1:
            cue_candidates = []
    for cue_path in cue_candidates:
        chapters = parse_cue(cue_path)
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

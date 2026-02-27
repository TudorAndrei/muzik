"""File-based cache compatible with the bash scripts.

Same directory (~/.cache/music-scripts) and key scheme (yt_<id>, chapters_<id>,
split_<audio_hash>_<chapters_hash>) so Python and bash share the cache transparently.
"""

import hashlib
import json
from datetime import datetime, timedelta
from pathlib import Path
from typing import Optional

from muzik.config import CACHE_DIR


# ---------------------------------------------------------------------------
# Internal helpers
# ---------------------------------------------------------------------------


def _path(key: str, ext: str = "txt") -> Path:
    return CACHE_DIR / f"{key}.{ext}"


# ---------------------------------------------------------------------------
# Basic get/set/exists/delete
# ---------------------------------------------------------------------------


def get(key: str, ext: str = "txt") -> Optional[str]:
    p = _path(key, ext)
    return p.read_text() if p.exists() else None


def set(key: str, value: str, ext: str = "txt") -> None:  # noqa: A001
    CACHE_DIR.mkdir(parents=True, exist_ok=True)
    _path(key, ext).write_text(value)


def get_json(key: str) -> Optional[dict]:
    raw = get(key, ext="json")
    if raw:
        try:
            return json.loads(raw)
        except json.JSONDecodeError:
            return None
    return None


def set_json(key: str, data: dict) -> None:
    set(key, json.dumps(data, indent=2), ext="json")  # noqa: A001


def exists(key: str, ext: str = "txt") -> bool:
    return _path(key, ext).exists()


def delete(key: str, ext: str = "txt") -> bool:
    p = _path(key, ext)
    if p.exists():
        p.unlink()
        return True
    return False


# ---------------------------------------------------------------------------
# Directory-level operations
# ---------------------------------------------------------------------------


def list_all() -> list[Path]:
    """Return all cache files sorted newest-first."""
    if not CACHE_DIR.exists():
        return []
    return sorted(
        (p for p in CACHE_DIR.iterdir() if p.is_file()),
        key=lambda p: p.stat().st_mtime,
        reverse=True,
    )


def total_size() -> int:
    """Total size of all cache files in bytes."""
    return sum(p.stat().st_size for p in list_all())


def clean(max_age_days: int = 30) -> int:
    """Remove empty files and files older than *max_age_days*. Returns count removed."""
    cutoff = datetime.now() - timedelta(days=max_age_days)
    removed = 0
    for p in list_all():
        mtime = datetime.fromtimestamp(p.stat().st_mtime)
        if p.stat().st_size == 0 or mtime < cutoff:
            p.unlink()
            removed += 1
    return removed


# ---------------------------------------------------------------------------
# Hashing helpers
# ---------------------------------------------------------------------------


def file_hash(path: Path) -> str:
    """SHA-256 hash of file contents (hex digest)."""
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


def split_cache_key(audio_path: Path, chapters_path: Path) -> str:
    """Cache key for a split operation — matches bash script scheme."""
    return f"split_{file_hash(audio_path)}_{file_hash(chapters_path)}"

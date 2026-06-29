"""File-based cache helpers."""

import hashlib
import json
from datetime import datetime, timedelta
from pathlib import Path
from typing import Any, Optional

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


def stable_hash(value: Any) -> str:
    """Return a short deterministic hash for source/cache identifiers."""
    try:
        raw = json.dumps(value, sort_keys=True, default=str, separators=(",", ":"))
    except TypeError:
        raw = str(value)
    return hashlib.sha256(raw.encode("utf-8")).hexdigest()[:16]


def download_cache_key(source: str, source_id: str) -> str:
    """Source-neutral cache key for a downloaded candidate."""
    return f"download_{source}_{stable_hash(source_id)}"


def workflow_cache_key(source: str, request: Any) -> str:
    """Source-neutral cache key for workflow resume state."""
    return f"workflow_{source}_{stable_hash(request)}"


def candidate_cache_key(candidate: Any) -> str:
    """Source-neutral cache key for a ranked source candidate."""
    if hasattr(candidate, "to_dict"):
        candidate = candidate.to_dict()
    source = (
        candidate.get("source", "unknown") if isinstance(candidate, dict) else "unknown"
    )
    return f"candidate_{source}_{stable_hash(candidate)}"

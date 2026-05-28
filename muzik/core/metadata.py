"""Source-neutral metadata sidecar helpers."""

from __future__ import annotations

import json
from datetime import datetime
from pathlib import Path
from typing import Any, Optional


MUZIK_METADATA_VERSION = 1
MUZIK_METADATA_FILENAME = ".muzik.json"


def metadata_sidecar_for(path: Path) -> Path:
    """Return the canonical `.muzik.json` sidecar path for a file or directory."""
    if path.suffix:
        return path.with_suffix(".muzik.json")
    return path / MUZIK_METADATA_FILENAME


def _candidate_paths(path: Path) -> list[Path]:
    if path.name.endswith(".muzik.json"):
        return [path]
    if path.suffix:
        base = path.with_suffix(".muzik.json")
        return [base, path.parent / MUZIK_METADATA_FILENAME]
    return [path / MUZIK_METADATA_FILENAME]


def read_muzik_metadata(path_or_root: Path) -> Optional[dict[str, Any]]:
    """Read source-neutral metadata for *path_or_root*, if present."""
    for path in _candidate_paths(path_or_root):
        if not path.exists():
            continue
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            continue
        if isinstance(data, dict):
            return data
    return None


def write_muzik_metadata(path_or_root: Path, data: dict[str, Any]) -> Path:
    """Write source-neutral metadata and return the sidecar path."""
    sidecar = metadata_sidecar_for(path_or_root)
    sidecar.parent.mkdir(parents=True, exist_ok=True)
    payload = {
        "version": MUZIK_METADATA_VERSION,
        "downloaded_at": datetime.now().isoformat(timespec="seconds"),
        **data,
    }
    sidecar.write_text(
        json.dumps(payload, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    return sidecar


def find_muzik_metadata(audio_path: Path) -> Optional[dict[str, Any]]:
    """Find `.muzik.json` metadata for an audio file."""
    return read_muzik_metadata(audio_path)

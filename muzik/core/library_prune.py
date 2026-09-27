"""Remove library rows whose audio files no longer exist."""

from __future__ import annotations

from pathlib import Path

from muzik.core.native_library import NativeLibrary


class PruneAborted(Exception):
    """Too many files are missing for a safe prune."""

    def __init__(self, missing: int, total: int) -> None:
        super().__init__(f"{missing}/{total} items missing")
        self.missing = missing
        self.total = total


def prune_missing_items(
    config_path: Path | None = None,
    *,
    safety_fraction: float = 0.5,
) -> int:
    removed, aborted = NativeLibrary(config_path).prune_missing_items(safety_fraction)
    if aborted is not None:
        raise PruneAborted(*aborted)
    return removed

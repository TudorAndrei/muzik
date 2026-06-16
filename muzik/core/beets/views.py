"""UI-safe view models for beets internals."""

from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
from typing import Any


@dataclass(frozen=True, slots=True)
class BeetsMatchView:
    candidate_id: str
    artist: str | None = None
    album: str | None = None
    title: str | None = None
    distance: float | None = None
    raw: Any = None


@dataclass(frozen=True, slots=True)
class BeetsTaskView:
    paths: list[Path] = field(default_factory=list)
    is_album: bool = False
    matches: list[BeetsMatchView] = field(default_factory=list)
    raw: Any = None


@dataclass(frozen=True, slots=True)
class BeetsDuplicateView:
    path: Path | None = None
    artist: str | None = None
    album: str | None = None
    title: str | None = None
    raw: Any = None


def task_view(task: Any) -> BeetsTaskView:
    paths = []
    for path in getattr(task, "paths", []) or []:
        try:
            paths.append(Path(path))
        except TypeError:
            paths.append(Path(str(path)))

    candidates = []
    for index, candidate in enumerate(getattr(task, "candidates", []) or []):
        candidates.append(match_view(candidate, index=index))

    return BeetsTaskView(
        paths=paths,
        is_album=bool(getattr(task, "is_album", False)),
        matches=candidates,
        raw=task,
    )


def match_view(candidate: Any, *, index: int = 0) -> BeetsMatchView:
    info = getattr(candidate, "info", candidate)
    return BeetsMatchView(
        candidate_id=str(getattr(candidate, "id", None) or index),
        artist=_field(info, "artist"),
        album=_field(info, "album"),
        title=_field(info, "title"),
        distance=_distance(candidate),
        raw=candidate,
    )


def duplicate_view(duplicate: Any) -> BeetsDuplicateView:
    path_value = _field(duplicate, "path")
    path = Path(path_value) if path_value else None
    return BeetsDuplicateView(
        path=path,
        artist=_field(duplicate, "artist"),
        album=_field(duplicate, "album"),
        title=_field(duplicate, "title"),
        raw=duplicate,
    )


def _field(obj: Any, name: str) -> str | None:
    if isinstance(obj, dict):
        value = obj.get(name)
    else:
        value = getattr(obj, name, None)
        if value is None and hasattr(obj, "get"):
            try:
                value = obj.get(name)
            except Exception:
                value = None
    return str(value) if value is not None else None


def _distance(candidate: Any) -> float | None:
    value = getattr(candidate, "distance", None)
    if value is None:
        return None
    try:
        return float(value)
    except (TypeError, ValueError):
        return None

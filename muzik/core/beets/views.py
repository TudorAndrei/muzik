"""UI-safe view models for beets internals."""

from __future__ import annotations

from dataclasses import dataclass, field, replace
import os
from pathlib import Path
from typing import Any, Sequence


@dataclass(frozen=True, slots=True)
class BeetsMatchView:
    candidate_id: str
    artist: str | None = None
    album: str | None = None
    title: str | None = None
    distance: float | None = None


@dataclass(frozen=True, slots=True)
class BeetsTaskView:
    task_id: str
    paths: list[Path] = field(default_factory=list)
    is_album: bool = False
    item_count: int = 0
    current_artist: str | None = None
    current_album: str | None = None
    current_year: str | None = None
    matches: list[BeetsMatchView] = field(default_factory=list)


@dataclass(frozen=True, slots=True)
class BeetsDuplicateView:
    path: Path | None = None
    artist: str | None = None
    album: str | None = None
    title: str | None = None


def task_view(
    task: Any,
    *,
    task_id: str,
    ranked: Sequence[tuple[int, float]] | None = None,
) -> BeetsTaskView:
    paths = []
    for path in getattr(task, "paths", []) or []:
        try:
            paths.append(Path(os.fsdecode(path)))
        except TypeError:
            paths.append(Path(str(path)))

    items = list(getattr(task, "items", []) or [])
    item_count = len(items)
    first_item = items[0] if item_count else None

    source_candidates = list(getattr(task, "candidates", []) or [])
    candidates = []
    for index, distance in (
        ranked
        if ranked is not None
        else [(index, None) for index in range(len(source_candidates))]
    ):
        match = match_view(
            source_candidates[index], candidate_id=f"{task_id}:match:{index}"
        )
        candidates.append(
            replace(match, distance=distance) if distance is not None else match
        )

    return BeetsTaskView(
        task_id=task_id,
        paths=paths,
        is_album=bool(getattr(task, "is_album", False)),
        item_count=item_count,
        current_artist=_field(first_item, "artist"),
        current_album=_field(first_item, "album"),
        current_year=_field(first_item, "year"),
        matches=candidates,
    )


def match_view(candidate: Any, *, candidate_id: str) -> BeetsMatchView:
    info = getattr(candidate, "info", candidate)
    return BeetsMatchView(
        candidate_id=candidate_id,
        artist=_field(info, "artist"),
        album=_field(info, "album"),
        title=_field(info, "title"),
        distance=_distance(candidate),
    )


def duplicate_view(duplicate: Any) -> BeetsDuplicateView:
    path_value = _field(duplicate, "path")
    path = Path(path_value) if path_value else None
    return BeetsDuplicateView(
        path=path,
        artist=_field(duplicate, "artist"),
        album=_field(duplicate, "album"),
        title=_field(duplicate, "title"),
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
    except TypeError, ValueError:
        return None

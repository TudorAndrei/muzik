"""Native music import and tag writing services."""

from __future__ import annotations

from collections.abc import Callable
import json
import os
from pathlib import Path
from typing import Any

from muzik import _native
from muzik.core.import_models import (
    ImportDecisions,
    ImportEventEmitter,
    ImportOptions,
    LogEvent,
    NonInteractiveImportDecisions,
    NullImportEventEmitter,
)
from muzik.core.library_lookup import resolve_item_path
from muzik.core.native_import import run_native_import
from muzik.core.native_library import NativeLibrary

TagOnlyRunner = Callable[[Path, ImportOptions], int]


class OrganizationError(RuntimeError):
    """An organization request cannot finish."""


def organize_paths(
    options: ImportOptions,
    *,
    tag_only: bool = False,
    decisions: ImportDecisions | None = None,
    events: ImportEventEmitter | None = None,
    tag_only_runner: TagOnlyRunner | None = None,
) -> None:
    """Import paths or write tags from library records."""
    for path in options.paths:
        if not path.exists():
            raise OrganizationError(f"Directory not found: {path}")
    if tag_only:
        runner = tag_only_runner or write_library_tags
        emitter = events or NullImportEventEmitter()
        for path in options.paths:
            count = runner(path, options)
            if options.dry_run:
                emitter.emit(
                    LogEvent(f"Tag preview: {count} library items under {path}.")
                )
        return
    import_paths(options, decisions=decisions, events=events)


def import_paths(
    options: ImportOptions,
    *,
    decisions: ImportDecisions | None = None,
    events: ImportEventEmitter | None = None,
) -> None:
    run_native_import(
        options.normalized(),
        decisions or NonInteractiveImportDecisions(quiet=options.quiet),
        events or NullImportEventEmitter(),
    )


def _native_fields(item: Any) -> dict[str, str]:
    """Map library item values to audio tag values."""
    names = (
        "title",
        "artist",
        "album",
        "albumartist",
        "track",
        "tracktotal",
        "disc",
        "disctotal",
        "mb_trackid",
        "mb_releasetrackid",
        "mb_workid",
        "mb_albumid",
        "mb_releasegroupid",
        "mb_artistid",
        "mb_albumartistid",
        "label",
        "catalognum",
        "country",
        "media",
        "albumdisambig",
        "rg_track_gain",
        "rg_track_peak",
        "rg_album_gain",
        "rg_album_peak",
    )
    fields: dict[str, str] = {}
    for name in names:
        value = item.get(name)
        if value is None or value == "":
            continue
        if name in {"rg_track_gain", "rg_album_gain"}:
            fields[name] = f"{float(value):.2f} dB"
        elif name in {"rg_track_peak", "rg_album_peak"}:
            fields[name] = f"{float(value):.6f}"
        else:
            fields[name] = str(value)
    for prefix, field in (("", "date"), ("original_", "original_date")):
        year = item.get(f"{prefix}year")
        month = item.get(f"{prefix}month")
        day = item.get(f"{prefix}day")
        if year:
            fields[field] = f"{int(year):04d}"
            if month:
                fields[field] += f"-{int(month):02d}"
                if day:
                    fields[field] += f"-{int(day):02d}"
    comp = item.get("comp")
    if comp is not None:
        fields["comp"] = "1" if comp else "0"
    return fields


def write_library_tags(path: Path, options: ImportOptions) -> int:
    """Write tags and local cover art for items under a library path."""
    library = NativeLibrary(options.config_path)
    requested = path.resolve()
    found = 0
    for item in library.items():
        item_path = resolve_item_path(
            os.fsdecode(library.directory), item.path
        ).resolve()
        if item_path != requested and not item_path.is_relative_to(requested):
            continue
        found += 1
        if options.dry_run:
            continue
        payload = json.dumps(
            {"fields": _native_fields(item), "lists": {}, "custom": {}}
        )
        try:
            _native.write_audio_tags(str(item_path), payload)
            cover = _native.find_audio_cover(str(item_path.parent))
            if cover:
                cover_path = Path(cover)
                mime = (
                    "image/png" if cover_path.suffix.lower() == ".png" else "image/jpeg"
                )
                _native.embed_audio_cover(str(item_path), cover_path.read_bytes(), mime)
        except Exception as exc:
            raise OrganizationError(
                f"native tag writer failed for {item_path}: {exc}"
            ) from exc
    if not found:
        raise OrganizationError(f"No library items match {path}")
    return found

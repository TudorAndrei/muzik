"""UI-neutral Beets organization service."""

from __future__ import annotations

from collections.abc import Callable
from typing import Any
import json
import logging
import os
from pathlib import Path
import subprocess
import sys

from muzik.config import get_native_settings
from muzik.core.beets.config import open_library
from muzik.core.beets.decisions import BeetsDecisions
from muzik.core.beets.events import BeetsEventEmitter
from muzik.core.beets.importer import ImportOptions, import_paths


TagOnlyRunner = Callable[[Path, ImportOptions], None]
_LOG = logging.getLogger(__name__)


class OrganizationError(RuntimeError):
    """Raised when an organization request cannot be performed."""


def organize_paths(
    options: ImportOptions,
    *,
    tag_only: bool = False,
    decisions: BeetsDecisions | None = None,
    events: BeetsEventEmitter | None = None,
    tag_only_runner: TagOnlyRunner | None = None,
) -> None:
    """Organize or tag paths without depending on a presentation framework."""
    for path in options.paths:
        if not path.exists():
            raise OrganizationError(f"Directory not found: {path}")

    if tag_only:
        if tag_only_runner is None:
            raise OrganizationError("Tag-only organization requires a runner.")
        for path in options.paths:
            tag_only_runner(path, options)
        return

    import_paths(options, decisions=decisions, events=events)


def _tag_only_beet(path: Path, options: ImportOptions) -> None:
    """Run Beets' isolated tag writer."""
    beet = Path(sys.executable).parent / "beet"
    command = [str(beet) if beet.exists() else "beet"]
    if options.config_path and options.config_path.exists():
        command.extend(["-c", str(options.config_path)])
    command.append("write")
    if not options.dry_run:
        command.append("--yes")
    command.append(str(path))
    result = subprocess.run(command)
    if result.returncode:
        raise OrganizationError(f"beet write exited with code {result.returncode}")


def _native_fields(item: Any) -> dict[str, str]:
    """Map the library item fields to mediafile tag values."""
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


def _tag_only_native(path: Path, options: ImportOptions) -> None:
    from muzik import _native

    library = open_library(options.config_path)
    requested = path.resolve()
    found = False
    for item in library.items():
        item_path = Path(os.fsdecode(item.path)).resolve()
        if item_path != requested and not item_path.is_relative_to(requested):
            continue
        found = True
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
                suffix = cover_path.suffix.lower()
                mime = "image/png" if suffix == ".png" else "image/jpeg"
                _native.embed_audio_cover(str(item_path), cover_path.read_bytes(), mime)
        except Exception as exc:
            raise OrganizationError(
                f"native tag writer failed for {item_path}: {exc}"
            ) from exc
    if not found:
        raise OrganizationError(f"No library items match {path}")


def tag_only_with_beet(path: Path, options: ImportOptions) -> None:
    """Write library tags through the selected backend."""
    mode = get_native_settings()["tags"]
    if mode == "native":
        _tag_only_native(path, options)
        return
    _tag_only_beet(path, options)
    if mode == "shadow" and not options.dry_run:
        try:
            from muzik import _native

            library = open_library(options.config_path)
            requested = path.resolve()
            for item in library.items():
                item_path = Path(os.fsdecode(item.path)).resolve()
                if item_path != requested and not item_path.is_relative_to(requested):
                    continue
                actual = _native.read_audio_tags(str(item_path))["fields"]
                expected = _native_fields(item)
                if any(
                    actual.get(key) != value
                    for key, value in expected.items()
                    if key in {"title", "artist", "album", "albumartist"}
                ):
                    _LOG.warning("native tag read differs from beets for %s", item_path)
        except Exception as exc:
            _LOG.warning("native tag read failed after beets write: %s", exc)

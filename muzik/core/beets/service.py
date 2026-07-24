"""UI-neutral Beets organization service."""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path
import subprocess
import sys

from muzik.core.beets.decisions import BeetsDecisions
from muzik.core.beets.events import BeetsEventEmitter
from muzik.core.beets.importer import ImportOptions, import_paths


TagOnlyRunner = Callable[[Path, ImportOptions], None]


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


def tag_only_with_beet(path: Path, options: ImportOptions) -> None:
    """Run Beets' isolated tag writer without binding it to a UI adapter."""
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

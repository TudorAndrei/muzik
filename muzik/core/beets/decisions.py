"""Decision protocols for beets imports."""

from __future__ import annotations

from enum import Enum
from pathlib import Path
from typing import Any, Protocol

from beets import importer

from muzik.core.beets.views import BeetsDuplicateView, BeetsTaskView


class BeetsDuplicateDecision(str, Enum):
    SKIP = "skip"
    KEEP_ALL = "keep_all"
    REMOVE_OLD = "remove_old"
    MERGE = "merge"


class BeetsMatchDecision(str, Enum):
    """Non-candidate choices available when resolving a Beets match."""

    AS_IS = "as_is"


class BeetsDecisions(Protocol):
    def should_resume_beets_import(self, path: Path) -> bool: ...

    def choose_beets_album_match(self, task: BeetsTaskView) -> Any: ...

    def choose_beets_track_match(self, task: BeetsTaskView) -> Any: ...

    def resolve_beets_duplicate(
        self,
        task: BeetsTaskView,
        duplicates: list[BeetsDuplicateView],
    ) -> BeetsDuplicateDecision: ...


class NonInteractiveBeetsDecisions:
    """Conservative default decisions for unattended imports."""

    def __init__(
        self,
        *,
        quiet: bool = False,
        duplicate_decision: BeetsDuplicateDecision = BeetsDuplicateDecision.SKIP,
    ) -> None:
        self.quiet = quiet
        self.duplicate_decision = duplicate_decision

    def should_resume_beets_import(self, path: Path) -> bool:
        return False

    def choose_beets_album_match(self, task: BeetsTaskView) -> Any:
        return importer.Action.SKIP if self.quiet else importer.Action.APPLY

    def choose_beets_track_match(self, task: BeetsTaskView) -> Any:
        return importer.Action.SKIP if self.quiet else importer.Action.APPLY

    def resolve_beets_duplicate(
        self,
        task: BeetsTaskView,
        duplicates: list[BeetsDuplicateView],
    ) -> BeetsDuplicateDecision:
        return self.duplicate_decision

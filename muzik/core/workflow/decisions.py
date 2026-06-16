"""Decision protocols for UI-neutral workflow code."""

from __future__ import annotations

from enum import Enum
from pathlib import Path
from typing import Protocol

from muzik.core.chapters import Chapter
from muzik.core.sources.base import Candidate


class ChapterDecision(str, Enum):
    ACCEPT = "accept"
    EDIT = "edit"
    REJECT = "reject"


class WorkflowDecisionError(ValueError):
    """Raised when a UI adapter cannot provide a valid workflow decision."""


class WorkflowDecisions(Protocol):
    def choose_soulseek_candidate(self, candidates: list[Candidate]) -> Candidate: ...

    def confirm_chapters(
        self,
        source: Path,
        chapters: list[Chapter],
    ) -> ChapterDecision: ...

    def edit_chapters(self, chapters: list[Chapter]) -> list[Chapter] | None: ...


class NonInteractiveWorkflowDecisions:
    """Deterministic workflow decisions for tests and unattended runs."""

    def __init__(
        self,
        *,
        candidate_index: int = 0,
        chapter_decision: ChapterDecision = ChapterDecision.ACCEPT,
    ) -> None:
        self.candidate_index = candidate_index
        self.chapter_decision = chapter_decision

    def choose_soulseek_candidate(self, candidates: list[Candidate]) -> Candidate:
        if not candidates:
            raise WorkflowDecisionError("No Soulseek candidates available.")
        if self.candidate_index < 0 or self.candidate_index >= len(candidates):
            raise WorkflowDecisionError("Soulseek candidate number out of range.")
        return candidates[self.candidate_index]

    def confirm_chapters(
        self,
        source: Path,
        chapters: list[Chapter],
    ) -> ChapterDecision:
        return self.chapter_decision

    def edit_chapters(self, chapters: list[Chapter]) -> list[Chapter] | None:
        return chapters

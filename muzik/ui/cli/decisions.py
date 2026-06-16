"""Terminal decision adapter for workflow operations."""

from __future__ import annotations

from pathlib import Path
from typing import Callable

import typer

from muzik.core.chapters import Chapter
from muzik.core.sources.base import Candidate
from muzik.core.workflow.decisions import ChapterDecision, WorkflowDecisionError
from muzik.ui.chapter_editor import display_chapter_table, edit_chapters
from muzik.ui.console import console


class CliWorkflowDecisions:
    def __init__(
        self,
        *,
        interactive: bool = True,
        candidate_limit: int = 5,
        candidate_prompt: str = "  Soulseek candidate",
        prompt: Callable[..., str] = typer.prompt,
        display_soulseek_candidates: bool = True,
    ) -> None:
        self.interactive = interactive
        self.candidate_limit = candidate_limit
        self.candidate_prompt = candidate_prompt
        self.prompt = prompt
        self.display_soulseek_candidates = display_soulseek_candidates

    def choose_soulseek_candidate(self, candidates: list[Candidate]) -> Candidate:
        if not candidates:
            raise WorkflowDecisionError("No Soulseek candidates available.")

        limit = min(self.candidate_limit, len(candidates))
        if self.display_soulseek_candidates:
            for idx, candidate in enumerate(candidates[:limit], 1):
                console.print(
                    f"  [dim]{idx}.[/dim] {candidate.title} "
                    f"[dim]score={candidate.score:.1f} "
                    f"files={len(candidate.files)} "
                    f"user={candidate.user or '?'}[/dim]"
                )

        choice = 1
        if self.interactive:
            raw = self.prompt(self.candidate_prompt, default="1")
            try:
                choice = int(raw)
            except ValueError as exc:
                raise WorkflowDecisionError(
                    "Invalid Soulseek candidate number."
                ) from exc

        if choice < 1 or choice > limit:
            raise WorkflowDecisionError("Soulseek candidate number out of range.")
        return candidates[choice - 1]

    def confirm_chapters(
        self,
        source: Path,
        chapters: list[Chapter],
    ) -> ChapterDecision:
        if not self.interactive:
            return ChapterDecision.ACCEPT
        try:
            raw = input("  Use these chapters? [Y/n/e=edit]: ").strip().lower() or "y"
        except (EOFError, KeyboardInterrupt):
            return ChapterDecision.REJECT

        if raw == "e":
            return ChapterDecision.EDIT
        if raw == "y":
            return ChapterDecision.ACCEPT
        return ChapterDecision.REJECT

    def edit_chapters(self, chapters: list[Chapter]) -> list[Chapter] | None:
        return edit_chapters(chapters)


class CliChapterReviewDecisions:
    """Decision adapter for the standalone chapter editor loop."""

    def choose_action(self, chapters: list[Chapter]) -> ChapterDecision:
        display_chapter_table(chapters)
        console.print(
            "\n  [bold][c][/bold]ontinue  [bold][e][/bold]dit  [bold][a][/bold]bort",
        )
        try:
            raw = input("  Choice [c]: ").strip().lower() or "c"
        except (EOFError, KeyboardInterrupt):
            return ChapterDecision.REJECT
        if raw == "c":
            return ChapterDecision.ACCEPT
        if raw == "e":
            return ChapterDecision.EDIT
        if raw == "a":
            return ChapterDecision.REJECT
        raise WorkflowDecisionError("Please enter c, e, or a.")

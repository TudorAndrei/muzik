"""Core protocol adapters for the desktop interface."""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path
from queue import Queue
from typing import Protocol, TypeVar

from muzik.core.beets.decisions import BeetsDuplicateDecision, BeetsMatchDecision
from muzik.core.beets.events import (
    BeetsDuplicateEvent,
    BeetsErrorEvent,
    BeetsEvent,
    BeetsImportFinishedEvent,
    BeetsImportStartedEvent,
    BeetsLogEvent,
    BeetsTaskEvent,
)
from muzik.core.beets.views import BeetsDuplicateView, BeetsTaskView
from muzik.core.chapters import Chapter
from muzik.core.sources.base import Candidate
from muzik.core.workflow.cancellation import CancellationToken
from muzik.core.workflow.decisions import ChapterDecision, WorkflowDecisionError
from muzik.core.workflow.events import (
    CandidatesFoundEvent,
    ChapterReviewRequestedEvent,
    ErrorEvent,
    MessageEvent,
    ProgressAdvancedEvent,
    ProgressFinishedEvent,
    ProgressStartedEvent,
    StepFinishedEvent,
    StepStartedEvent,
    WorkflowEvent,
)
from muzik.gui import modals
from muzik.gui.bridge import GuiBridge
from muzik.gui.pipeline import PipelineView


ResultT = TypeVar("ResultT")


class DecisionBridge(Protocol):
    """Small bridge interface used by blocking decision adapters."""

    def request(
        self,
        show_modal: Callable[[Queue[ResultT]], None],
        cancellation: CancellationToken,
    ) -> ResultT: ...


class GuiWorkflowEventEmitter:
    """Map core workflow events to queued pipeline updates."""

    def __init__(
        self,
        bridge: GuiBridge,
        pipeline: PipelineView,
        cancellation: CancellationToken,
    ) -> None:
        self.bridge = bridge
        self.pipeline = pipeline
        self.cancellation = cancellation

    def emit(self, event: WorkflowEvent) -> None:
        if self.cancellation.is_cancelled() or self.bridge.is_shutdown():
            return

        def update() -> None:
            if self.cancellation.is_cancelled():
                return
            if isinstance(event, StepStartedEvent):
                detail = f": {event.detail}" if event.detail else ""
                self.pipeline.set_status(f"{event.name}{detail}")
                self.pipeline.log(f"Started {event.name}{detail}")
            elif isinstance(event, StepFinishedEvent):
                detail = f": {event.detail}" if event.detail else ""
                status = "finished" if event.success else "failed"
                self.pipeline.log(f"{event.name} {status}{detail}")
                self.pipeline.finish_step()
            elif isinstance(event, CandidatesFoundEvent):
                self.pipeline.load_candidates(event.candidates)
                self.pipeline.log(
                    f"{len(event.candidates)} {event.source} candidate(s)."
                )
            elif isinstance(event, ChapterReviewRequestedEvent):
                self.pipeline.load_chapters(event.chapters)
                self.pipeline.log(f"Chapter review requested for {event.source.name}.")
            elif isinstance(event, ProgressStartedEvent):
                self.pipeline.start_progress(event.total, event.description)
            elif isinstance(event, ProgressAdvancedEvent):
                self.pipeline.advance_progress(
                    event.advance,
                    event.completed,
                    event.total,
                )
            elif isinstance(event, ProgressFinishedEvent):
                self.pipeline.finish_progress(event.task_id)
            elif isinstance(event, MessageEvent):
                self.pipeline.set_status(event.message)
                self.pipeline.log(event.message)
            elif isinstance(event, ErrorEvent):
                prefix = "Fatal" if event.fatal else "Error"
                self.pipeline.log(f"{prefix}: {event.message}")

        self.bridge.submit(update)


class GuiBeetsEventEmitter:
    """Map safe Beets events to queued pipeline updates."""

    def __init__(
        self,
        bridge: GuiBridge,
        pipeline: PipelineView,
        cancellation: CancellationToken,
    ) -> None:
        self.bridge = bridge
        self.pipeline = pipeline
        self.cancellation = cancellation

    def emit(self, event: BeetsEvent) -> None:
        if self.cancellation.is_cancelled() or self.bridge.is_shutdown():
            return

        def update() -> None:
            if self.cancellation.is_cancelled():
                return
            if isinstance(event, BeetsTaskEvent):
                self.pipeline.load_beets_task(event.task)
                self.pipeline.log(f"Beets match requested for {event.task.task_id}.")
            elif isinstance(event, BeetsDuplicateEvent):
                self.pipeline.log(f"Beets found {len(event.duplicates)} duplicate(s).")
            elif isinstance(event, BeetsImportStartedEvent):
                self.pipeline.log(
                    f"Beets import started for {len(event.paths)} path(s)."
                )
            elif isinstance(event, BeetsImportFinishedEvent):
                status = "finished" if event.success else "failed"
                self.pipeline.log(f"Beets import {status}.")
            elif isinstance(event, BeetsLogEvent):
                self.pipeline.log(event.message)
            elif isinstance(event, BeetsErrorEvent):
                self.pipeline.log(f"Beets error: {event.message}")

        self.bridge.submit(update)


class GuiWorkflowDecisions:
    """Provide workflow decisions through render-thread modals."""

    def __init__(
        self,
        bridge: DecisionBridge,
        *,
        interactive: bool = True,
        cancellation: CancellationToken | None = None,
    ) -> None:
        self.bridge = bridge
        self.interactive = interactive
        self.cancellation = cancellation or CancellationToken()

    def choose_soulseek_candidate(self, candidates: list[Candidate]) -> Candidate:
        self.cancellation.raise_if_cancelled()
        if not candidates:
            raise WorkflowDecisionError("No Soulseek candidates available.")
        if not self.interactive:
            return candidates[0]
        selected = self.bridge.request(
            lambda result: modals.candidate_modal(candidates, result),
            self.cancellation,
        )
        if selected is None:
            raise WorkflowDecisionError("No Soulseek candidate selected.")
        return selected

    def confirm_chapters(
        self,
        source: Path,
        chapters: list[Chapter],
    ) -> ChapterDecision:
        self.cancellation.raise_if_cancelled()
        if not self.interactive:
            return ChapterDecision.ACCEPT
        return self.bridge.request(
            lambda result: modals.chapter_review_modal(source, chapters, result),
            self.cancellation,
        )

    def edit_chapters(self, chapters: list[Chapter]) -> list[Chapter] | None:
        self.cancellation.raise_if_cancelled()
        if not self.interactive:
            return chapters

        def show(result: Queue[list[Chapter] | None]) -> None:
            modals.chapter_edit_modal(chapters, result)

        return self.bridge.request(show, self.cancellation)


class GuiBeetsDecisions:
    """Provide Beets decisions through render-thread modals."""

    def __init__(
        self,
        bridge: DecisionBridge,
        *,
        interactive: bool = True,
        cancellation: CancellationToken | None = None,
    ) -> None:
        self.bridge = bridge
        self.interactive = interactive
        self.cancellation = cancellation or CancellationToken()

    def should_resume_beets_import(self, path: Path) -> bool:
        return False

    def choose_beets_album_match(
        self,
        task: BeetsTaskView,
    ) -> str | BeetsMatchDecision | None:
        return self._choose_match(task)

    def choose_beets_track_match(
        self,
        task: BeetsTaskView,
    ) -> str | BeetsMatchDecision | None:
        return self._choose_match(task)

    def _choose_match(
        self,
        task: BeetsTaskView,
    ) -> str | BeetsMatchDecision | None:
        self.cancellation.raise_if_cancelled()
        if not self.interactive:
            return BeetsMatchDecision.AS_IS

        def show(
            result: Queue[str | BeetsMatchDecision | None],
        ) -> None:
            modals.beets_match_modal(task, result)

        return self.bridge.request(show, self.cancellation)

    def resolve_beets_duplicate(
        self,
        task: BeetsTaskView,
        duplicates: list[BeetsDuplicateView],
    ) -> BeetsDuplicateDecision:
        self.cancellation.raise_if_cancelled()
        if not self.interactive:
            return BeetsDuplicateDecision.SKIP
        return self.bridge.request(
            lambda result: modals.duplicate_modal(duplicates, result),
            self.cancellation,
        )

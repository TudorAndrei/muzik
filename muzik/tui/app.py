"""Textual application entrypoint."""

from __future__ import annotations

from collections.abc import Callable, Iterable
import inspect
from pathlib import Path
from typing import cast

from textual.app import App, ComposeResult, SystemCommand
from textual.containers import Container, Horizontal, Vertical
from textual.screen import Screen
from textual.widgets import Button, Footer, Header, Log, ProgressBar, Static
from textual.worker import Worker, WorkerState

from muzik.core.beets.decisions import BeetsDuplicateDecision
from muzik.core.beets.events import (
    BeetsDuplicateEvent,
    BeetsErrorEvent,
    BeetsEvent,
    BeetsImportFinishedEvent,
    BeetsImportStartedEvent,
    BeetsLogEvent,
    BeetsTaskEvent,
)
from muzik.core.beets.views import BeetsTaskView
from muzik.core.chapters import Chapter
from muzik.core.sources.base import Candidate
from muzik.core.workflow.decisions import (
    ChapterDecision,
    WorkflowDecisionError,
    WorkflowDecisions,
)
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
    WorkflowEventEmitter,
)
from muzik.core.workflow.cancellation import CancellationToken, WorkflowCancelled
from muzik.core.workflow.service import (
    WorkflowOptions,
    WorkflowRequest,
    WorkflowRunOperations,
    WorkflowServiceError,
    run_workflow,
)
from muzik.core.workflow.operations import build_workflow_operations
from muzik.tui.screens import (
    BeetsMatchScreen,
    CandidateSelectionScreen,
    ChapterEditScreen,
    ChapterReviewScreen,
    DuplicateResolutionScreen,
    WorkflowLaunchConfig,
    WorkflowLauncherScreen,
)
from muzik.tui.widgets import BeetsMatchTable, CandidateTable, ChapterTable


WorkflowOperationsFactory = Callable[..., WorkflowRunOperations]


class TuiWorkflowEventEmitter:
    """Bridge workflow events from worker threads into the Textual screen."""

    def __init__(
        self,
        screen: "PipelineScreen",
        cancellation: CancellationToken,
    ) -> None:
        self.screen = screen
        self.cancellation = cancellation

    def emit(self, event: WorkflowEvent) -> None:
        if self.cancellation.is_cancelled() or not self.screen.is_mounted:
            return
        try:
            self.screen.app.call_from_thread(self.screen.handle_workflow_event, event)
        except RuntimeError:
            self.screen.handle_workflow_event(event)


class TuiWorkflowDecisions:
    """Workflow decisions backed by Textual modals."""

    def __init__(
        self,
        screen: "PipelineScreen",
        *,
        interactive: bool = True,
        cancellation: CancellationToken | None = None,
    ) -> None:
        self.screen = screen
        self.interactive = interactive
        self.cancellation = cancellation or CancellationToken()

    def choose_soulseek_candidate(self, candidates: list[Candidate]) -> Candidate:
        self.cancellation.raise_if_cancelled()
        if not candidates:
            raise WorkflowDecisionError("No Soulseek candidates available.")
        if not self.interactive:
            return candidates[0]
        self.screen.app.call_from_thread(
            self.screen.handle_workflow_event,
            MessageEvent("Waiting for Soulseek candidate selection."),
        )
        selected = self.screen.app.call_from_thread(
            self.screen.request_candidate_choice,
            candidates,
        )
        if selected is None:
            raise WorkflowDecisionError("No Soulseek candidate selected.")
        self.cancellation.raise_if_cancelled()
        return cast(Candidate, selected)

    def confirm_chapters(
        self,
        source: Path,
        chapters: list[Chapter],
    ) -> ChapterDecision:
        self.cancellation.raise_if_cancelled()
        if not self.interactive:
            return ChapterDecision.ACCEPT
        decision = self.screen.app.call_from_thread(
            self.screen.request_chapter_decision,
            source,
            chapters,
        )
        self.cancellation.raise_if_cancelled()
        return cast(ChapterDecision, decision)

    def edit_chapters(self, chapters: list[Chapter]) -> list[Chapter] | None:
        self.cancellation.raise_if_cancelled()
        if not self.interactive:
            return chapters
        edited = self.screen.app.call_from_thread(
            self.screen.request_chapter_edit,
            chapters,
        )
        self.cancellation.raise_if_cancelled()
        return cast(list[Chapter] | None, edited)


class TuiBeetsDecisions:
    """Beets decisions backed by Textual modals and opaque candidate IDs."""

    def __init__(
        self, screen: "PipelineScreen", cancellation: CancellationToken
    ) -> None:
        self.screen = screen
        self.cancellation = cancellation

    def should_resume_beets_import(self, path: Path) -> bool:
        return False

    def choose_beets_album_match(self, task: BeetsTaskView) -> str | None:
        return self._choose_match(task)

    def choose_beets_track_match(self, task: BeetsTaskView) -> str | None:
        return self._choose_match(task)

    def _choose_match(self, task: BeetsTaskView) -> str | None:
        self.cancellation.raise_if_cancelled()
        choice = self.screen.app.call_from_thread(
            self.screen.request_beets_match,
            task,
        )
        self.cancellation.raise_if_cancelled()
        return cast(str | None, choice)

    def resolve_beets_duplicate(
        self,
        task: BeetsTaskView,
        duplicates,
    ) -> BeetsDuplicateDecision:
        self.cancellation.raise_if_cancelled()
        decision = self.screen.app.call_from_thread(
            self.screen.request_duplicate_decision,
            duplicates,
        )
        self.cancellation.raise_if_cancelled()
        return cast(BeetsDuplicateDecision, decision or BeetsDuplicateDecision.SKIP)


class TuiBeetsEventEmitter:
    """Bridge safe Beets events from an importer worker into the pipeline UI."""

    def __init__(
        self, screen: "PipelineScreen", cancellation: CancellationToken
    ) -> None:
        self.screen = screen
        self.cancellation = cancellation

    def emit(self, event: BeetsEvent) -> None:
        if self.cancellation.is_cancelled() or not self.screen.is_mounted:
            return
        try:
            self.screen.app.call_from_thread(self.screen.handle_beets_event, event)
        except RuntimeError:
            self.screen.handle_beets_event(event)


class PipelineScreen(Screen[None]):
    """Run and render a workflow pipeline."""

    CSS = """
    PipelineScreen {
        background: $surface;
    }

    #pipeline {
        width: 100%;
        height: 100%;
        padding: 1 2;
    }

    #pipeline-top {
        height: auto;
        margin-bottom: 1;
    }

    #status {
        width: 1fr;
        content-align: left middle;
    }

    #workflow-progress {
        width: 35;
    }

    #pipeline-tables {
        height: 2fr;
        margin-bottom: 1;
    }

    #pipeline-tables > Vertical {
        width: 1fr;
        margin-right: 1;
    }

    #pipeline-log {
        height: 1fr;
    }

    .screen-title {
        text-style: bold;
        color: $accent;
        height: 1;
        margin-bottom: 1;
    }

    #pipeline-actions {
        dock: bottom;
        height: 3;
    }
    """

    def __init__(
        self,
        config: WorkflowLaunchConfig,
        *,
        operations_factory: WorkflowOperationsFactory,
    ) -> None:
        super().__init__()
        self.config = config
        self.operations_factory = operations_factory
        self._workflow_worker: Worker[None] | None = None
        self._cancellation = CancellationToken()
        self._return_to_launcher = False

    def compose(self) -> ComposeResult:
        yield Header(show_clock=True)
        with Container(id="pipeline"):
            with Horizontal(id="pipeline-top"):
                yield Static("Ready", id="status")
                yield ProgressBar(total=4, id="workflow-progress")
            with Horizontal(id="pipeline-tables"):
                with Vertical():
                    yield Static("Source candidates", classes="screen-title")
                    yield CandidateTable(id="pipeline-candidates")
                with Vertical():
                    yield Static("Chapters", classes="screen-title")
                    yield ChapterTable(id="pipeline-chapters")
                with Vertical():
                    yield Static("Beets matches", classes="screen-title")
                    yield BeetsMatchTable(id="pipeline-beets")
            yield Log(id="pipeline-log")
        with Horizontal(id="pipeline-actions"):
            yield Button("Back", id="back")
            yield Button("Quit", id="quit")
        yield Footer()

    def on_mount(self) -> None:
        self._log(f"Workflow: {self.config.raw}")
        self._workflow_worker = self.run_worker(
            self._run_workflow,
            name="workflow",
            thread=True,
            exit_on_error=False,
        )

    async def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == "back":
            if not self._request_return_to_launcher():
                await cast("MuzikTuiApp", self.app).open_launcher()
        elif event.button.id == "quit":
            self.app.exit()

    def _request_return_to_launcher(self) -> bool:
        """Request cancellation and report whether worker teardown is pending."""
        if self._workflow_worker is None or self._workflow_worker.state in {
            WorkerState.SUCCESS,
            WorkerState.ERROR,
            WorkerState.CANCELLED,
        }:
            return False
        self._return_to_launcher = True
        self._cancellation.cancel()
        self.query_one("#status", Static).update("Cancelling…")
        self.query_one("#back", Button).disabled = True
        return True

    def on_worker_state_changed(self, event: Worker.StateChanged) -> None:
        if event.worker.name != "workflow":
            return
        if self._return_to_launcher and event.state in {
            WorkerState.SUCCESS,
            WorkerState.ERROR,
            WorkerState.CANCELLED,
        }:
            self.app.call_later(cast("MuzikTuiApp", self.app).open_launcher)
            return
        if event.state == WorkerState.SUCCESS:
            self.query_one("#status", Static).update("Complete")
            self._log("Workflow complete.")
            return
        if event.state == WorkerState.ERROR:
            self.query_one("#status", Static).update("Failed")
            error = event.worker.error
            self._log(f"Workflow failed: {error}")
        elif event.state == WorkerState.CANCELLED:
            self.query_one("#status", Static).update("Cancelled")
            self._log("Workflow cancelled.")

    async def request_candidate_choice(
        self,
        candidates: list[Candidate],
    ) -> Candidate | None:
        return await self.app.push_screen_wait(CandidateSelectionScreen(candidates))

    async def request_chapter_decision(
        self,
        source: Path,
        chapters: list[Chapter],
    ) -> ChapterDecision:
        return await self.app.push_screen_wait(ChapterReviewScreen(source, chapters))

    async def request_chapter_edit(
        self,
        chapters: list[Chapter],
    ) -> list[Chapter] | None:
        return await self.app.push_screen_wait(ChapterEditScreen(chapters))

    async def request_beets_match(self, task: BeetsTaskView) -> str | None:
        return await self.app.push_screen_wait(BeetsMatchScreen(task))

    async def request_duplicate_decision(self, duplicates) -> BeetsDuplicateDecision:
        return await self.app.push_screen_wait(DuplicateResolutionScreen(duplicates))

    def handle_workflow_event(self, event: WorkflowEvent) -> None:
        if isinstance(event, StepStartedEvent):
            detail = f": {event.detail}" if event.detail else ""
            self.query_one("#status", Static).update(f"{event.name}{detail}")
            self._log(f"Started {event.name}{detail}")
        elif isinstance(event, StepFinishedEvent):
            detail = f": {event.detail}" if event.detail else ""
            status = "finished" if event.success else "failed"
            self._log(f"{event.name} {status}{detail}")
            self.query_one("#workflow-progress", ProgressBar).advance(1)
        elif isinstance(event, CandidatesFoundEvent):
            self.query_one("#pipeline-candidates", CandidateTable).load_candidates(
                event.candidates
            )
            self._log(f"{len(event.candidates)} {event.source} candidate(s).")
        elif isinstance(event, ChapterReviewRequestedEvent):
            self.query_one("#pipeline-chapters", ChapterTable).load_chapters(
                event.chapters
            )
            self._log(f"Chapter review requested for {event.source.name}.")
        elif isinstance(event, ProgressStartedEvent):
            progress = self.query_one("#workflow-progress", ProgressBar)
            progress.update(total=event.total)
            self._log(event.description)
        elif isinstance(event, ProgressAdvancedEvent):
            progress = self.query_one("#workflow-progress", ProgressBar)
            if event.completed is None:
                progress.update(total=event.total, advance=event.advance)
            else:
                progress.update(total=event.total, progress=event.completed)
        elif isinstance(event, ProgressFinishedEvent):
            self._log(f"Progress {event.task_id} finished.")
        elif isinstance(event, MessageEvent):
            self.query_one("#status", Static).update(event.message)
            self._log(event.message)
        elif isinstance(event, ErrorEvent):
            prefix = "Fatal" if event.fatal else "Error"
            self._log(f"{prefix}: {event.message}")

    def handle_beets_event(self, event: BeetsEvent) -> None:
        if isinstance(event, BeetsTaskEvent):
            self.query_one("#pipeline-beets", BeetsMatchTable).load_task(event.task)
            self._log(f"Beets match requested for {event.task.task_id}.")
        elif isinstance(event, BeetsDuplicateEvent):
            self._log(f"Beets found {len(event.duplicates)} duplicate(s).")
        elif isinstance(event, BeetsImportStartedEvent):
            self._log(f"Beets import started for {len(event.paths)} path(s).")
        elif isinstance(event, BeetsImportFinishedEvent):
            status = "finished" if event.success else "failed"
            self._log(f"Beets import {status}.")
        elif isinstance(event, BeetsLogEvent):
            self._log(event.message)
        elif isinstance(event, BeetsErrorEvent):
            self._log(f"Beets error: {event.message}")

    def _run_workflow(self) -> None:
        request = WorkflowRequest(
            raw=self.config.raw,
            output=self.config.output,
            splits=self.config.splits,
        )
        options = WorkflowOptions(
            review=self.config.review,
            no_split=self.config.no_split,
            no_organize=self.config.no_organize,
            import_=self.config.import_,
            tag_only=self.config.tag_only,
            dry_run=self.config.dry_run,
            jobs=self.config.jobs,
            config=self.config.config,
            keep_source=self.config.keep_source,
            force=self.config.force,
            metadata_source=self.config.metadata_source,
            audio_source=self.config.audio_source,
            prefer=self.config.prefer,
            fallback=self.config.fallback,
            interactive=self.config.interactive,
        )
        events = TuiWorkflowEventEmitter(self, self._cancellation)
        beets_events = TuiBeetsEventEmitter(self, self._cancellation)
        beets_decisions = TuiBeetsDecisions(self, self._cancellation)
        decisions = TuiWorkflowDecisions(
            self,
            interactive=self.config.interactive,
            cancellation=self._cancellation,
        )
        parameters = inspect.signature(self.operations_factory).parameters.values()
        supports_beets_adapters = (
            any(
                parameter.kind == inspect.Parameter.VAR_POSITIONAL
                or parameter.kind == inspect.Parameter.VAR_KEYWORD
                for parameter in parameters
            )
            or len(inspect.signature(self.operations_factory).parameters) >= 5
        )
        if supports_beets_adapters:
            operations = self.operations_factory(
                self.config,
                decisions,
                events,
                beets_decisions,
                beets_events,
            )
        else:
            operations = self.operations_factory(self.config, decisions, events)
        try:
            run_workflow(
                request,
                options,
                operations=operations,
                events=events,
                cancellation=self._cancellation,
            )
        except WorkflowCancelled:
            return
        except WorkflowServiceError as exc:
            events.emit(ErrorEvent(exc.message, fatal=True))
            raise

    def _log(self, line: str) -> None:
        self.query_one("#pipeline-log", Log).write_line(line)


class MuzikTuiApp(App[None]):
    """Top-level Textual app."""

    TITLE = "muzik"
    SUB_TITLE = "workflow"
    CSS = """
    .screen-title {
        text-style: bold;
        color: $accent;
    }
    """

    BINDINGS = [
        ("ctrl+p", "command_palette", "Palette"),
        ("q", "quit", "Quit"),
    ]

    def __init__(
        self,
        *,
        operations_factory: WorkflowOperationsFactory | None = None,
    ) -> None:
        super().__init__()
        self.operations_factory = operations_factory or _default_operations

    def on_mount(self) -> None:
        self.push_screen(WorkflowLauncherScreen(), self._open_pipeline)

    async def open_launcher(self) -> None:
        if isinstance(self.screen, PipelineScreen):
            if self.screen._request_return_to_launcher():
                return
            await self.pop_screen()
        if not isinstance(self.screen, WorkflowLauncherScreen):
            self.push_screen(WorkflowLauncherScreen(), self._open_pipeline)

    def get_system_commands(self, screen: Screen) -> Iterable[SystemCommand]:
        yield from super().get_system_commands(screen)
        if isinstance(screen, WorkflowLauncherScreen):
            yield SystemCommand(
                "Run workflow",
                "Start the workflow with the current launcher values",
                lambda: screen.dismiss(screen.read_config()),
            )
        elif isinstance(screen, PipelineScreen):
            yield SystemCommand(
                "Back to launcher",
                "Return to the workflow launcher",
                self.open_launcher,
            )

    def _open_pipeline(self, config: WorkflowLaunchConfig | None) -> None:
        if config is None:
            return
        if not config.raw:
            self.notify("Enter a URL or local path.", severity="warning")
            self.push_screen(WorkflowLauncherScreen(), self._open_pipeline)
            return
        self.push_screen(
            PipelineScreen(config, operations_factory=self.operations_factory)
        )


def _default_operations(
    config: WorkflowLaunchConfig,
    decisions: WorkflowDecisions,
    events: WorkflowEventEmitter,
    beets_decisions: TuiBeetsDecisions | None = None,
    beets_events: TuiBeetsEventEmitter | None = None,
) -> WorkflowRunOperations:
    options = WorkflowOptions(
        review=config.review,
        no_split=config.no_split,
        no_organize=config.no_organize,
        import_=config.import_,
        tag_only=config.tag_only,
        dry_run=config.dry_run,
        jobs=config.jobs,
        config=config.config,
        keep_source=config.keep_source,
        force=config.force,
        metadata_source=config.metadata_source,
        audio_source=config.audio_source,
        prefer=config.prefer,
        fallback=config.fallback,
        interactive=config.interactive,
    )
    return build_workflow_operations(
        splits=config.splits,
        options=options,
        decisions=decisions,
        events=events,
        beets_decisions=beets_decisions,
        beets_events=beets_events,
    )


def tui_cmd() -> None:
    """Open the Textual workflow UI."""
    MuzikTuiApp().run()

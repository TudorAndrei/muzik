"""Textual application entrypoint."""

from __future__ import annotations

from collections.abc import Callable, Iterable
from pathlib import Path
from typing import cast

import typer
from textual.app import App, ComposeResult, SystemCommand
from textual.containers import Container, Horizontal, Vertical
from textual.screen import Screen
from textual.widgets import Button, Footer, Header, Log, ProgressBar, Static
from textual.worker import Worker, WorkerState

from muzik.commands.download import download_cmd
from muzik.commands.workflow import (
    _acquire_from_soulseek,
    _get_playlist_video_ids,
    _prepopulate_archive,
    _process_audio_files,
)
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
from muzik.core.workflow.service import (
    WorkflowOptions,
    WorkflowRequest,
    WorkflowRunOperations,
    WorkflowServiceError,
    run_workflow,
)
from muzik.tui.screens import (
    CandidateSelectionScreen,
    ChapterEditScreen,
    ChapterReviewScreen,
    WorkflowLaunchConfig,
    WorkflowLauncherScreen,
)
from muzik.tui.widgets import BeetsMatchTable, CandidateTable, ChapterTable


WorkflowOperationsFactory = Callable[
    [WorkflowLaunchConfig, WorkflowDecisions, WorkflowEventEmitter],
    WorkflowRunOperations,
]


class TuiWorkflowEventEmitter:
    """Bridge workflow events from worker threads into the Textual screen."""

    def __init__(self, screen: "PipelineScreen") -> None:
        self.screen = screen

    def emit(self, event: WorkflowEvent) -> None:
        try:
            self.screen.app.call_from_thread(self.screen.handle_workflow_event, event)
        except RuntimeError:
            self.screen.handle_workflow_event(event)


class TuiWorkflowDecisions:
    """Workflow decisions backed by Textual modals."""

    def __init__(self, screen: "PipelineScreen", *, interactive: bool = True) -> None:
        self.screen = screen
        self.interactive = interactive

    def choose_soulseek_candidate(self, candidates: list[Candidate]) -> Candidate:
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
        return cast(Candidate, selected)

    def confirm_chapters(
        self,
        source: Path,
        chapters: list[Chapter],
    ) -> ChapterDecision:
        if not self.interactive:
            return ChapterDecision.ACCEPT
        decision = self.screen.app.call_from_thread(
            self.screen.request_chapter_decision,
            source,
            chapters,
        )
        return cast(ChapterDecision, decision)

    def edit_chapters(self, chapters: list[Chapter]) -> list[Chapter] | None:
        if not self.interactive:
            return chapters
        edited = self.screen.app.call_from_thread(
            self.screen.request_chapter_edit,
            chapters,
        )
        return cast(list[Chapter] | None, edited)


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
            if self._workflow_worker is not None:
                self._workflow_worker.cancel()
            await cast("MuzikTuiApp", self.app).open_launcher()
        elif event.button.id == "quit":
            self.app.exit()

    def on_worker_state_changed(self, event: Worker.StateChanged) -> None:
        if event.worker.name != "workflow":
            return
        if event.state == WorkerState.SUCCESS:
            self.query_one("#status", Static).update("Complete")
            self._log("Workflow complete.")
            return
        if event.state == WorkerState.ERROR:
            self.query_one("#status", Static).update("Failed")
            error = event.worker.error
            self._log(f"Workflow failed: {error}")

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
        events = TuiWorkflowEventEmitter(self)
        decisions = TuiWorkflowDecisions(self, interactive=self.config.interactive)
        operations = self.operations_factory(self.config, decisions, events)
        try:
            run_workflow(request, options, operations=operations, events=events)
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
) -> WorkflowRunOperations:
    def download_audio(url: str, output: Path, archive_file: Path | None) -> bool:
        try:
            download_cmd(
                url=url,
                output=output,
                format="bestaudio",
                quality="0",
                no_chapters=False,
                archive_file=archive_file,
            )
        except (SystemExit, typer.Exit) as exc:
            return _exit_code(exc) == 0
        return True

    def process_audio(audio_inputs: list[Path], pre_split_dirs: list[Path]) -> None:
        _process_audio_files(
            audio_inputs=audio_inputs,
            pre_split_dirs=pre_split_dirs,
            splits=config.splits,
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
            decisions=decisions,
            events=events,
        )

    return WorkflowRunOperations(
        download_audio=download_audio,
        process_audio=process_audio,
        acquire_soulseek=lambda raw: _acquire_from_soulseek(
            raw,
            prefer=config.prefer,
            interactive=config.interactive,
            fallback=config.fallback,
            decisions=decisions,
            events=events,
        ),
        prepopulate_archive=_prepopulate_archive,
        get_playlist_video_ids=_get_playlist_video_ids,
    )


def _exit_code(exc: BaseException) -> int:
    return int(getattr(exc, "exit_code", getattr(exc, "code", 1)) or 0)


def tui_cmd() -> None:
    """Open the Textual workflow UI."""
    MuzikTuiApp().run()

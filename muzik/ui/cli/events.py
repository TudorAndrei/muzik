"""Rich renderer for structured workflow events."""

from __future__ import annotations

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
from muzik.ui.chapter_editor import display_chapter_table
from muzik.ui.console import console, err


class RichWorkflowEventRenderer:
    def emit(self, event: WorkflowEvent) -> None:
        if isinstance(event, MessageEvent):
            self._message(event)
        elif isinstance(event, StepStartedEvent):
            detail = f" [dim]{event.detail}[/dim]" if event.detail else ""
            console.print(f"[bold]{event.name}[/bold]{detail}")
        elif isinstance(event, StepFinishedEvent):
            style = "green" if event.success else "red"
            detail = f" [dim]{event.detail}[/dim]" if event.detail else ""
            console.print(f"[{style}]{event.name} complete[/{style}]{detail}")
        elif isinstance(event, CandidatesFoundEvent):
            console.print(
                f"[dim]{len(event.candidates)} {event.source} candidate(s) found.[/dim]"
            )
        elif isinstance(event, ChapterReviewRequestedEvent):
            display_chapter_table(event.chapters, title=event.title or "Chapters")
        elif isinstance(event, ErrorEvent):
            prefix = "[red]Error:[/red]" if event.fatal else "[yellow]Warning:[/yellow]"
            err(f"{prefix} {event.message}")
        elif isinstance(
            event,
            (ProgressStartedEvent, ProgressAdvancedEvent, ProgressFinishedEvent),
        ):
            return None

    def _message(self, event: MessageEvent) -> None:
        if event.severity == "error":
            err(f"[red]{event.message}[/red]")
        elif event.severity == "warning":
            console.print(f"[yellow]{event.message}[/yellow]")
        elif event.severity == "debug":
            console.print(f"[dim]{event.message}[/dim]")
        else:
            console.print(event.message)

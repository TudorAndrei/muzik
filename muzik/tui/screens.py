"""Textual screens for the muzik TUI."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
import re

from textual import on
from textual.app import ComposeResult
from textual.containers import Container, Horizontal, ScrollableContainer, Vertical
from textual.screen import ModalScreen, Screen
from textual.widgets import (
    Button,
    Footer,
    Header,
    Input,
    Label,
    Select,
    Static,
    Switch,
    TextArea,
)

from muzik.config import DEFAULT_DOWNLOAD_DIR, DEFAULT_SPLITS_DIR
from muzik.core.beets.decisions import BeetsDuplicateDecision
from muzik.core.beets.views import BeetsDuplicateView, BeetsTaskView
from muzik.core.chapters import Chapter, serialize_chapters
from muzik.core.sources.base import Candidate
from muzik.core.workflow.decisions import ChapterDecision
from muzik.tui.widgets import (
    BeetsMatchTable,
    CandidateTable,
    ChapterTable,
    DuplicateTable,
)


@dataclass(frozen=True, slots=True)
class WorkflowLaunchConfig:
    raw: str
    output: Path = DEFAULT_DOWNLOAD_DIR
    splits: Path = DEFAULT_SPLITS_DIR
    review: bool = False
    no_split: bool = False
    no_organize: bool = False
    import_: bool = False
    tag_only: bool = False
    dry_run: bool = False
    jobs: int = 0
    config: Path | None = None
    keep_source: bool = False
    force: bool = False
    metadata_source: str = "auto"
    audio_source: str = "youtube"
    prefer: str = "lossless"
    fallback: str = "youtube"
    interactive: bool = True


class WorkflowLauncherScreen(Screen[WorkflowLaunchConfig]):
    """Collect workflow options before opening the pipeline screen."""

    CSS = """
    WorkflowLauncherScreen {
        background: $surface;
    }

    #launcher {
        width: 100%;
        height: 100%;
        padding: 1 2;
    }

    .section {
        width: 100%;
        height: auto;
        margin-bottom: 1;
    }

    .field-label {
        width: 18;
        color: $text-muted;
    }

    .row {
        height: auto;
        margin-bottom: 1;
    }

    Input, Select {
        width: 1fr;
    }

    .switch-row {
        width: 1fr;
        height: 3;
        margin-right: 2;
    }

    .switch-row Label {
        width: 18;
    }

    #actions {
        height: 3;
        dock: bottom;
    }
    """

    def compose(self) -> ComposeResult:
        yield Header(show_clock=True)
        with ScrollableContainer(id="launcher"):
            yield Static("Workflow", classes="screen-title")
            with Vertical(classes="section"):
                yield _input_row("URL or path", "raw", "")
                yield _input_row("Downloads", "output", str(DEFAULT_DOWNLOAD_DIR))
                yield _input_row("Splits", "splits", str(DEFAULT_SPLITS_DIR))
                yield _input_row("Beets config", "config", "")
            with Vertical(classes="section"):
                with Horizontal(classes="row"):
                    yield Label("Audio source", classes="field-label")
                    yield Select.from_values(
                        ["youtube", "soulseek", "auto"],
                        value="youtube",
                        allow_blank=False,
                        id="audio-source",
                    )
                with Horizontal(classes="row"):
                    yield Label("Metadata", classes="field-label")
                    yield Select.from_values(
                        ["auto", "youtube", "musicbrainz", "none"],
                        value="auto",
                        allow_blank=False,
                        id="metadata-source",
                    )
                with Horizontal(classes="row"):
                    yield Label("Prefer", classes="field-label")
                    yield Select.from_values(
                        ["lossless", "best", "mp3", "flac"],
                        value="lossless",
                        allow_blank=False,
                        id="prefer",
                    )
                with Horizontal(classes="row"):
                    yield Label("Fallback", classes="field-label")
                    yield Select.from_values(
                        ["youtube", "none"],
                        value="youtube",
                        allow_blank=False,
                        id="fallback",
                    )
                yield _input_row("Jobs", "jobs", "0")
            with Vertical(classes="section"):
                with Horizontal(classes="row"):
                    yield _switch_row("Review chapters", "review", False)
                    yield _switch_row("No split", "no-split", False)
                    yield _switch_row("No organize", "no-organize", False)
                with Horizontal(classes="row"):
                    yield _switch_row("Import", "import", False)
                    yield _switch_row("Tag only", "tag-only", False)
                    yield _switch_row("Dry run", "dry-run", False)
                with Horizontal(classes="row"):
                    yield _switch_row("Keep source", "keep-source", False)
                    yield _switch_row("Force", "force", False)
                    yield _switch_row("Interactive", "interactive", True)
        with Horizontal(id="actions"):
            yield Button("Run", variant="primary", id="run")
            yield Button("Quit", id="quit")
        yield Footer()

    @on(Button.Pressed, "#run")
    def run_workflow(self) -> None:
        self.dismiss(self.read_config())

    @on(Button.Pressed, "#quit")
    def quit_app(self) -> None:
        self.app.exit()

    def read_config(self) -> WorkflowLaunchConfig:
        raw = self.query_one("#raw", Input).value.strip()
        jobs_raw = self.query_one("#jobs", Input).value.strip()
        config_raw = self.query_one("#config", Input).value.strip()
        return WorkflowLaunchConfig(
            raw=raw,
            output=Path(self.query_one("#output", Input).value).expanduser(),
            splits=Path(self.query_one("#splits", Input).value).expanduser(),
            review=self.query_one("#review", Switch).value,
            no_split=self.query_one("#no-split", Switch).value,
            no_organize=self.query_one("#no-organize", Switch).value,
            import_=self.query_one("#import", Switch).value,
            tag_only=self.query_one("#tag-only", Switch).value,
            dry_run=self.query_one("#dry-run", Switch).value,
            jobs=int(jobs_raw or "0"),
            config=Path(config_raw).expanduser() if config_raw else None,
            keep_source=self.query_one("#keep-source", Switch).value,
            force=self.query_one("#force", Switch).value,
            metadata_source=_select_text(self.query_one("#metadata-source", Select)),
            audio_source=_select_text(self.query_one("#audio-source", Select)),
            prefer=_select_text(self.query_one("#prefer", Select)),
            fallback=_select_text(self.query_one("#fallback", Select)),
            interactive=self.query_one("#interactive", Switch).value,
        )


class CandidateSelectionScreen(ModalScreen[Candidate | None]):
    """Modal source candidate selector."""

    CSS = """
    CandidateSelectionScreen {
        align: center middle;
    }

    #candidate-dialog {
        width: 90%;
        height: 80%;
        background: $panel;
        border: round $accent;
        padding: 1;
    }

    #candidate-actions {
        dock: bottom;
        height: 3;
    }
    """

    def __init__(self, candidates: list[Candidate]) -> None:
        super().__init__()
        self.candidates = candidates

    def compose(self) -> ComposeResult:
        with Container(id="candidate-dialog"):
            yield Static("Soulseek candidates", classes="screen-title")
            yield CandidateTable(id="candidate-table")
            with Horizontal(id="candidate-actions"):
                yield Button("Use selected", variant="primary", id="select-candidate")
                yield Button("Skip", id="skip-candidate")

    def on_mount(self) -> None:
        self.query_one("#candidate-table", CandidateTable).load_candidates(
            self.candidates
        )

    @on(Button.Pressed, "#select-candidate")
    def select_candidate(self) -> None:
        table = self.query_one("#candidate-table", CandidateTable)
        index = max(0, min(table.cursor_row, len(self.candidates) - 1))
        self.dismiss(self.candidates[index])

    @on(Button.Pressed, "#skip-candidate")
    def skip_candidate(self) -> None:
        self.dismiss(None)


class ChapterReviewScreen(ModalScreen[ChapterDecision]):
    """Modal chapter table review."""

    CSS = """
    ChapterReviewScreen {
        align: center middle;
    }

    #chapter-dialog {
        width: 90%;
        height: 85%;
        background: $panel;
        border: round $accent;
        padding: 1;
    }

    #chapter-actions {
        dock: bottom;
        height: 3;
    }
    """

    def __init__(
        self,
        source: Path,
        chapters: list[Chapter],
        *,
        title: str | None = None,
    ) -> None:
        super().__init__()
        self.source = source
        self.chapters = chapters
        self._dialog_title = title or source.name

    def compose(self) -> ComposeResult:
        with Container(id="chapter-dialog"):
            yield Static(self._dialog_title, classes="screen-title")
            yield ChapterTable(id="chapter-table")
            with Horizontal(id="chapter-actions"):
                yield Button("Accept", variant="primary", id="accept-chapters")
                yield Button("Edit", id="edit-chapters")
                yield Button("Reject", variant="error", id="reject-chapters")

    def on_mount(self) -> None:
        self.query_one("#chapter-table", ChapterTable).load_chapters(self.chapters)

    @on(Button.Pressed, "#accept-chapters")
    def accept(self) -> None:
        self.dismiss(ChapterDecision.ACCEPT)

    @on(Button.Pressed, "#edit-chapters")
    def edit(self) -> None:
        self.dismiss(ChapterDecision.EDIT)

    @on(Button.Pressed, "#reject-chapters")
    def reject(self) -> None:
        self.dismiss(ChapterDecision.REJECT)


class ChapterEditScreen(ModalScreen[list[Chapter] | None]):
    """Text editor for chapter sidecar-style content."""

    CSS = """
    ChapterEditScreen {
        align: center middle;
    }

    #chapter-edit-dialog {
        width: 90%;
        height: 85%;
        background: $panel;
        border: round $accent;
        padding: 1;
    }

    #chapter-editor {
        height: 1fr;
    }

    #chapter-edit-actions {
        dock: bottom;
        height: 3;
    }
    """

    def __init__(self, chapters: list[Chapter]) -> None:
        super().__init__()
        self.chapters = chapters

    def compose(self) -> ComposeResult:
        with Container(id="chapter-edit-dialog"):
            yield Static("Edit chapters", classes="screen-title")
            yield TextArea(
                serialize_chapters(self.chapters),
                id="chapter-editor",
                show_line_numbers=True,
            )
            with Horizontal(id="chapter-edit-actions"):
                yield Button("Save", variant="primary", id="save-chapters")
                yield Button("Cancel", id="cancel-chapters")

    @on(Button.Pressed, "#save-chapters")
    def save(self) -> None:
        text = self.query_one("#chapter-editor", TextArea).text
        self.dismiss(_parse_chapter_text(text))

    @on(Button.Pressed, "#cancel-chapters")
    def cancel(self) -> None:
        self.dismiss(None)


class BeetsMatchScreen(ModalScreen[str | None]):
    """Modal beets match selector."""

    CSS = """
    BeetsMatchScreen {
        align: center middle;
    }

    #beets-match-dialog {
        width: 90%;
        height: 80%;
        background: $panel;
        border: round $accent;
        padding: 1;
    }

    #beets-match-actions {
        dock: bottom;
        height: 3;
    }
    """

    def __init__(self, task: BeetsTaskView) -> None:
        super().__init__()
        self._task_view = task

    def compose(self) -> ComposeResult:
        with Container(id="beets-match-dialog"):
            yield Static("Beets matches", classes="screen-title")
            yield BeetsMatchTable(id="beets-match-table")
            with Horizontal(id="beets-match-actions"):
                yield Button("Use selected", variant="primary", id="select-match")
                yield Button("Skip", id="skip-match")

    def on_mount(self) -> None:
        self.query_one("#beets-match-table", BeetsMatchTable).load_task(self._task_view)

    @on(Button.Pressed, "#select-match")
    def select_match(self) -> None:
        table = self.query_one("#beets-match-table", BeetsMatchTable)
        matches = self._task_view.matches
        if not matches:
            self.dismiss(None)
            return
        index = max(0, min(table.cursor_row, len(matches) - 1))
        self.dismiss(matches[index].candidate_id)

    @on(Button.Pressed, "#skip-match")
    def skip_match(self) -> None:
        self.dismiss(None)


class DuplicateResolutionScreen(ModalScreen[BeetsDuplicateDecision]):
    """Modal beets duplicate resolver."""

    CSS = """
    DuplicateResolutionScreen {
        align: center middle;
    }

    #duplicate-dialog {
        width: 90%;
        height: 80%;
        background: $panel;
        border: round $accent;
        padding: 1;
    }

    #duplicate-actions {
        dock: bottom;
        height: 3;
    }
    """

    def __init__(self, duplicates: list[BeetsDuplicateView]) -> None:
        super().__init__()
        self.duplicates = duplicates

    def compose(self) -> ComposeResult:
        with Container(id="duplicate-dialog"):
            yield Static("Duplicates", classes="screen-title")
            yield DuplicateTable(id="duplicate-table")
            with Horizontal(id="duplicate-actions"):
                yield Button("Skip", id="duplicate-skip")
                yield Button("Keep all", id="duplicate-keep")
                yield Button("Remove old", variant="warning", id="duplicate-remove")
                yield Button("Merge", variant="primary", id="duplicate-merge")

    def on_mount(self) -> None:
        self.query_one("#duplicate-table", DuplicateTable).load_duplicates(
            self.duplicates
        )

    @on(Button.Pressed, "#duplicate-skip")
    def skip(self) -> None:
        self.dismiss(BeetsDuplicateDecision.SKIP)

    @on(Button.Pressed, "#duplicate-keep")
    def keep_all(self) -> None:
        self.dismiss(BeetsDuplicateDecision.KEEP_ALL)

    @on(Button.Pressed, "#duplicate-remove")
    def remove_old(self) -> None:
        self.dismiss(BeetsDuplicateDecision.REMOVE_OLD)

    @on(Button.Pressed, "#duplicate-merge")
    def merge(self) -> None:
        self.dismiss(BeetsDuplicateDecision.MERGE)


def _input_row(label: str, id_: str, value: str) -> Horizontal:
    return Horizontal(
        Label(label, classes="field-label"),
        Input(value=value, id=id_),
        classes="row",
    )


def _switch_row(label: str, id_: str, value: bool) -> Horizontal:
    return Horizontal(
        Label(label),
        Switch(value=value, id=id_),
        classes="switch-row",
    )


def _select_text(select: Select) -> str:
    value = select.value
    return str(value) if value is not Select.NULL else ""


_CHAPTER_RE = re.compile(r"^(\d+):(\d{2})(?::(\d{2}))?\s+(.+)$")


def _parse_chapter_text(text: str) -> list[Chapter]:
    raw: list[tuple[int, str]] = []
    for line in text.splitlines():
        match = _CHAPTER_RE.match(line.strip())
        if not match:
            continue
        first, second, third, title = match.groups()
        if third is None:
            start = int(first) * 60 + int(second)
        else:
            start = int(first) * 3600 + int(second) * 60 + int(third)
        raw.append((start, title.strip()))

    chapters: list[Chapter] = []
    for index, (start, title) in enumerate(raw):
        end = raw[index + 1][0] if index + 1 < len(raw) else None
        chapters.append(Chapter(index=index + 1, start=start, end=end, title=title))
    return chapters

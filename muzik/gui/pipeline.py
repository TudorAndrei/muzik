"""Pipeline status and table view for the desktop interface."""

from __future__ import annotations

from collections.abc import Callable, Iterable
from queue import Full, Queue
from typing import Any

import dearpygui.dearpygui as dpg

from muzik.core.beets.decisions import BeetsMatchDecision
from muzik.core.beets.views import BeetsMatchView, BeetsTaskView
from muzik.core.chapters import Chapter
from muzik.core.sources.base import Candidate
from muzik.gui.theme import ACCENT, FAIL_COLOR, bind_primary_button


PIPELINE_WINDOW = "pipeline-window"
STATUS = "pipeline-status"
PIPELINE_ERROR = "pipeline-error"
PROGRESS = "pipeline-progress"
LOG = "pipeline-log"
CANDIDATE_TABLE = "pipeline-candidates"
CHAPTER_TABLE = "pipeline-chapters"
BEETS_TABLE = "pipeline-beets"
PIPELINE_OVERVIEW = "pipeline-overview"
BEETS_DECISIONS = "pipeline-beets-decisions"
BEETS_MATCHES = "pipeline-beets-matches"
BEETS_SOURCE = "pipeline-beets-source"
BEETS_CURRENT_TAGS = "pipeline-beets-current-tags"
BEETS_IMPORT_AS_IS = "pipeline-beets-import-as-is"
BEETS_SKIP = "pipeline-beets-skip"
BACK_BUTTON = "pipeline-back"
PIPELINE_BUSY = "pipeline-busy"
PIPELINE_BUSY_TEXT = "pipeline-busy-text"


class PipelineView:
    """Own the render-thread widgets for one workflow run."""

    def __init__(self, on_back, on_quit) -> None:
        self._on_back = on_back
        self._on_quit = on_quit
        self._progress_total: float | None = 4
        self._progress_value = 0.0
        self._log_lines: list[str] = []

    def build(self, raw: str) -> None:
        with dpg.window(
            tag=PIPELINE_WINDOW,
            label="muzik workflow",
            on_close=self._on_back,
        ):
            with dpg.group(horizontal=True):
                dpg.add_loading_indicator(
                    tag=PIPELINE_BUSY,
                    style=1,
                    circle_count=8,
                    speed=1.0,
                    radius=3,
                    thickness=2,
                    color=ACCENT,
                    secondary_color=(64, 70, 80),
                )
                dpg.add_text("Working...", tag=PIPELINE_BUSY_TEXT)
                dpg.add_text("Ready", tag=STATUS)
                dpg.add_progress_bar(
                    default_value=0.0,
                    overlay="0 / 4",
                    tag=PROGRESS,
                    width=-1,
                )
            dpg.add_text(
                "",
                tag=PIPELINE_ERROR,
                color=FAIL_COLOR,
                show=False,
                wrap=1500,
            )
            with dpg.group(horizontal=True, tag=PIPELINE_OVERVIEW):
                self._add_table(
                    "Source candidates",
                    CANDIDATE_TABLE,
                    ("Score", "Title", "User", "Format", "Files", "Path"),
                )
                self._add_table(
                    "Chapters",
                    CHAPTER_TABLE,
                    ("#", "Start", "End", "Title", "Duration"),
                )
                self._add_table(
                    "Beets matches",
                    BEETS_TABLE,
                    ("Action", "ID", "Artist", "Album", "Title", "Distance"),
                )
            with dpg.child_window(tag=BEETS_DECISIONS, show=False, height=440):
                dpg.add_text("Choose the album", color=ACCENT)
                dpg.add_text("", tag=BEETS_SOURCE, wrap=1100)
                dpg.add_text(
                    "Beets did not find one clear match. Review the choices "
                    "before muzik changes the track tags.",
                    wrap=1100,
                )
                dpg.add_separator()
                dpg.add_text("Current tags", color=ACCENT)
                dpg.add_text("", tag=BEETS_CURRENT_TAGS, wrap=1100)
                dpg.add_text(
                    "Keep these tags when the Beets matches are not the same album.",
                    wrap=1100,
                )
                with dpg.group(horizontal=True):
                    dpg.add_button(
                        tag=BEETS_IMPORT_AS_IS,
                        label="Keep current tags",
                        width=220,
                        height=40,
                    )
                    dpg.add_button(
                        tag=BEETS_SKIP,
                        label="Skip this album",
                        width=220,
                        height=40,
                    )
                dpg.add_separator()
                dpg.add_text("Possible Beets matches", color=ACCENT)
                with dpg.group(tag=BEETS_MATCHES):
                    pass
            bind_primary_button(BEETS_IMPORT_AS_IS)
            dpg.add_input_text(
                tag=LOG,
                multiline=True,
                readonly=True,
                height=-70,
                width=-1,
            )
            with dpg.group(horizontal=True):
                dpg.add_button(
                    label="Back",
                    callback=self._on_back,
                    tag=BACK_BUTTON,
                    width=100,
                )
                dpg.add_button(label="Quit", callback=self._on_quit, width=100)
        dpg.set_primary_window(PIPELINE_WINDOW, True)
        self.log(f"Workflow: {raw}")

    def destroy(self) -> None:
        if dpg.does_item_exist(PIPELINE_WINDOW):
            dpg.delete_item(PIPELINE_WINDOW)

    def set_status(self, value: str) -> None:
        dpg.set_value(STATUS, value)

    def set_busy(self, busy: bool) -> None:
        """Show motion and text while the worker is active."""
        if busy:
            dpg.show_item(PIPELINE_BUSY)
            dpg.show_item(PIPELINE_BUSY_TEXT)
        else:
            dpg.hide_item(PIPELINE_BUSY)
            dpg.hide_item(PIPELINE_BUSY_TEXT)

    def show_error(self, message: str) -> None:
        """Show a workflow error outside the detail log."""
        dpg.set_value(PIPELINE_ERROR, message)
        dpg.show_item(PIPELINE_ERROR)
        self.set_status("Workflow stopped.")

    def log(self, line: str) -> None:
        self._log_lines.append(line)
        dpg.set_value(LOG, "\n".join(self._log_lines))

    def start_progress(self, total: int | float | None, description: str) -> None:
        self._progress_total = float(total) if total else None
        self._progress_value = 0.0
        self._render_progress()
        self.log(description)

    def advance_progress(
        self,
        advance: int | float,
        completed: int | float | None,
        total: int | float | None,
    ) -> None:
        if total is not None:
            self._progress_total = float(total)
        self._progress_value = (
            float(completed)
            if completed is not None
            else self._progress_value + advance
        )
        self._render_progress()

    def finish_progress(self, task_id: str) -> None:
        if self._progress_total is not None:
            self._progress_value = self._progress_total
        self._render_progress()
        self.log(f"Progress {task_id} finished.")

    def finish_step(self) -> None:
        self._progress_value += 1
        self._render_progress()

    def load_candidates(self, candidates: Iterable[Candidate]) -> None:
        rows = []
        for candidate in candidates:
            rows.append(
                (
                    f"{candidate.score:.0f}",
                    candidate.title,
                    candidate.user or "",
                    candidate.quality.format or "",
                    str(len(candidate.files)),
                    candidate.path or candidate.source_id,
                )
            )
        self._replace_rows(CANDIDATE_TABLE, rows)

    def load_chapters(self, chapters: Iterable[Chapter]) -> None:
        self._replace_rows(
            CHAPTER_TABLE,
            [
                (
                    str(chapter.index),
                    chapter.start_ts,
                    chapter.end_ts or "",
                    chapter.title,
                    chapter.duration_str,
                )
                for chapter in chapters
            ],
        )

    def load_beets_task(self, task: BeetsTaskView) -> None:
        self.load_beets_matches(task.matches)

    def load_beets_matches(self, matches: Iterable[BeetsMatchView]) -> None:
        self._replace_rows(
            BEETS_TABLE,
            [
                (
                    "",
                    match.candidate_id,
                    match.artist or "",
                    match.album or "",
                    match.title or "",
                    "" if match.distance is None else f"{match.distance:.3f}",
                )
                for match in matches
            ],
        )

    def request_beets_match(
        self,
        task: BeetsTaskView,
        result: Queue[str | BeetsMatchDecision | None],
    ) -> None:
        """Show Beets choices in the pipeline and return the selected value."""
        dpg.delete_item(BEETS_MATCHES, children_only=True, slot=1)
        dpg.set_value(BEETS_SOURCE, self._beets_source_text(task))
        dpg.set_value(BEETS_CURRENT_TAGS, self._beets_current_tags(task))
        for match in task.matches:
            with dpg.group(parent=BEETS_MATCHES, horizontal=True):
                with dpg.group():
                    dpg.add_text(self._beets_match_title(match), wrap=850)
                    dpg.add_text(self._beets_match_detail(match), wrap=850)
                dpg.add_button(
                    label="Use this match",
                    width=180,
                    height=44,
                    callback=self._beets_choice_callback(
                        task,
                        result,
                        match.candidate_id,
                    ),
                )
            dpg.add_separator(parent=BEETS_MATCHES)

        if not task.matches:
            dpg.add_text(
                "Beets found no matches.",
                parent=BEETS_MATCHES,
            )

        dpg.configure_item(
            BEETS_IMPORT_AS_IS,
            callback=self._beets_choice_callback(
                task,
                result,
                BeetsMatchDecision.AS_IS,
            ),
        )
        dpg.configure_item(
            BEETS_SKIP,
            callback=self._beets_choice_callback(task, result, None),
        )
        self.set_status("Choose an album match.")
        dpg.hide_item(PIPELINE_OVERVIEW)
        dpg.show_item(BEETS_DECISIONS)

    def _beets_choice_callback(
        self,
        task: BeetsTaskView,
        result: Queue[str | BeetsMatchDecision | None],
        value: str | BeetsMatchDecision | None,
    ) -> Callable[..., None]:
        def select(
            sender: Any = None,
            app_data: Any = None,
            user_data: Any = None,
        ) -> None:
            try:
                result.put_nowait(value)
            except Full:
                return
            self.load_beets_matches(task.matches)
            dpg.hide_item(BEETS_DECISIONS)
            dpg.show_item(PIPELINE_OVERVIEW)
            self.set_status("Applying Beets choice...")

        return select

    @staticmethod
    def _beets_source_text(task: BeetsTaskView) -> str:
        if not task.paths:
            return "Current workflow album"
        first = task.paths[0]
        album = first.parent.name if first.suffix else first.name
        count = task.item_count or len(task.paths)
        unit = "track" if count == 1 else "tracks"
        return f"{album} · {count} {unit}"

    @staticmethod
    def _beets_match_title(match: BeetsMatchView) -> str:
        artist = match.artist or "Unknown artist"
        release = match.album or match.title or "Unknown album"
        return f"{artist} — {release}"

    @staticmethod
    def _beets_match_detail(match: BeetsMatchView) -> str:
        if match.distance is None:
            return "Difference: not available"
        quality = "Good match" if match.distance <= 0.2 else "Low confidence"
        return f"{quality} · Difference {match.distance:.3f}"

    @staticmethod
    def _beets_current_tags(task: BeetsTaskView) -> str:
        artist = task.current_artist or "Unknown artist"
        album = task.current_album or "Unknown album"
        year = f" · {task.current_year}" if task.current_year else ""
        return f"{artist} — {album}{year}"

    def disable_back(self) -> None:
        dpg.disable_item(BACK_BUTTON)

    def _render_progress(self) -> None:
        total = self._progress_total
        if total is None or total <= 0:
            dpg.set_value(PROGRESS, 0.0)
            dpg.configure_item(PROGRESS, overlay=f"{self._progress_value:g}")
            return
        fraction = max(0.0, min(1.0, self._progress_value / total))
        dpg.set_value(PROGRESS, fraction)
        dpg.configure_item(
            PROGRESS,
            overlay=f"{self._progress_value:g} / {total:g}",
        )

    @staticmethod
    def _add_table(label: str, tag: str, columns: tuple[str, ...]) -> None:
        with dpg.child_window(width=400, height=260):
            dpg.add_text(label, color=ACCENT)
            with dpg.table(
                tag=tag,
                header_row=True,
                resizable=True,
                policy=dpg.mvTable_SizingStretchProp,
                scrollY=True,
                height=220,
            ):
                for column in columns:
                    dpg.add_table_column(label=column)

    @staticmethod
    def _replace_rows(tag: str, rows: Iterable[tuple[Any, ...]]) -> None:
        dpg.delete_item(tag, children_only=True, slot=1)
        for row in rows:
            with dpg.table_row(parent=tag):
                for value in row:
                    dpg.add_text(str(value))

"""Pipeline status and table view for the desktop interface."""

from __future__ import annotations

from collections.abc import Iterable
from typing import Any

import dearpygui.dearpygui as dpg

from muzik.core.beets.views import BeetsMatchView, BeetsTaskView
from muzik.core.chapters import Chapter
from muzik.core.sources.base import Candidate
from muzik.gui.theme import ACCENT


PIPELINE_WINDOW = "pipeline-window"
STATUS = "pipeline-status"
PROGRESS = "pipeline-progress"
LOG = "pipeline-log"
CANDIDATE_TABLE = "pipeline-candidates"
CHAPTER_TABLE = "pipeline-chapters"
BEETS_TABLE = "pipeline-beets"
BACK_BUTTON = "pipeline-back"


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
                dpg.add_text("Ready", tag=STATUS)
                dpg.add_progress_bar(
                    default_value=0.0,
                    overlay="0 / 4",
                    tag=PROGRESS,
                    width=-1,
                )
            with dpg.group(horizontal=True):
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
                    ("ID", "Artist", "Album", "Title", "Distance"),
                )
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
                    match.candidate_id,
                    match.artist or "",
                    match.album or "",
                    match.title or "",
                    "" if match.distance is None else f"{match.distance:.3f}",
                )
                for match in matches
            ],
        )

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

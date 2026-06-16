"""Reusable Textual widgets for workflow and beets state."""

from __future__ import annotations

from typing import Iterable

from textual.widgets import DataTable

from muzik.core.beets.views import BeetsDuplicateView, BeetsMatchView, BeetsTaskView
from muzik.core.chapters import Chapter
from muzik.core.sources.base import Candidate


class CandidateTable(DataTable):
    """Table for source candidates."""

    def on_mount(self) -> None:
        self.cursor_type = "row"
        if not self.columns:
            self.add_columns("Score", "Title", "User", "Format", "Files", "Path")

    def load_candidates(self, candidates: Iterable[Candidate]) -> None:
        self.clear()
        for index, candidate in enumerate(candidates):
            quality = candidate.quality.format or ""
            self.add_row(
                f"{candidate.score:.0f}",
                candidate.title,
                candidate.user or "",
                quality,
                str(len(candidate.files)),
                candidate.path or candidate.source_id,
                key=str(index),
            )


class ChapterTable(DataTable):
    """Table for detected or edited chapters."""

    def on_mount(self) -> None:
        self.cursor_type = "row"
        if not self.columns:
            self.add_columns("#", "Start", "End", "Title", "Duration")

    def load_chapters(self, chapters: Iterable[Chapter]) -> None:
        self.clear()
        for chapter in chapters:
            self.add_row(
                str(chapter.index),
                chapter.start_ts,
                chapter.end_ts or "",
                chapter.title,
                chapter.duration_str,
                key=str(chapter.index),
            )


class BeetsMatchTable(DataTable):
    """Table for beets album and track matches."""

    def on_mount(self) -> None:
        self.cursor_type = "row"
        if not self.columns:
            self.add_columns("ID", "Artist", "Album", "Title", "Distance")

    def load_task(self, task: BeetsTaskView) -> None:
        self.load_matches(task.matches)

    def load_matches(self, matches: Iterable[BeetsMatchView]) -> None:
        self.clear()
        for match in matches:
            distance = "" if match.distance is None else f"{match.distance:.3f}"
            self.add_row(
                match.candidate_id,
                match.artist or "",
                match.album or "",
                match.title or "",
                distance,
                key=match.candidate_id,
            )


class DuplicateTable(DataTable):
    """Table for beets duplicates."""

    def on_mount(self) -> None:
        self.cursor_type = "row"
        if not self.columns:
            self.add_columns("Artist", "Album", "Title", "Path")

    def load_duplicates(self, duplicates: Iterable[BeetsDuplicateView]) -> None:
        self.clear()
        for index, duplicate in enumerate(duplicates):
            self.add_row(
                duplicate.artist or "",
                duplicate.album or "",
                duplicate.title or "",
                str(duplicate.path or ""),
                key=str(index),
            )

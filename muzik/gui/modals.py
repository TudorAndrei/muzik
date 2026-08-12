"""Blocking decision modal builders for the desktop interface."""

from __future__ import annotations

from pathlib import Path
from queue import Full, Queue
from typing import Any

import dearpygui.dearpygui as dpg

from muzik.core.beets.decisions import BeetsDuplicateDecision, BeetsMatchDecision
from muzik.core.beets.views import BeetsDuplicateView, BeetsTaskView
from muzik.core.chapters import Chapter, parse_chapters, serialize_chapters
from muzik.core.sources.base import Candidate
from muzik.core.workflow.decisions import ChapterDecision


_OPEN_MODALS: set[Any] = set()


def candidate_modal(
    candidates: list[Candidate],
    result: Queue[Candidate | None],
) -> None:
    tag = dpg.generate_uuid()
    _OPEN_MODALS.add(tag)
    with dpg.window(
        label="Soulseek candidates",
        tag=tag,
        modal=True,
        width=900,
        height=520,
        on_close=_result_callback(tag, result, None),
    ):
        with dpg.table(header_row=True, resizable=True, scrollY=True, height=420):
            for label in ("", "Score", "Title", "User", "Format", "Files", "Path"):
                dpg.add_table_column(label=label)
            for candidate in candidates:
                with dpg.table_row():
                    dpg.add_button(
                        label="Use",
                        callback=_result_callback(tag, result, candidate),
                    )
                    for value in (
                        f"{candidate.score:.0f}",
                        candidate.title,
                        candidate.user or "",
                        candidate.quality.format or "",
                        str(len(candidate.files)),
                        candidate.path or candidate.source_id,
                    ):
                        dpg.add_text(value)
        dpg.add_button(
            label="Skip",
            callback=_result_callback(tag, result, None),
        )


def chapter_review_modal(
    source: Path,
    chapters: list[Chapter],
    result: Queue[ChapterDecision],
) -> None:
    tag = dpg.generate_uuid()
    _OPEN_MODALS.add(tag)
    with dpg.window(
        label=f"Review chapters - {source.name}",
        tag=tag,
        modal=True,
        width=850,
        height=500,
        on_close=_result_callback(tag, result, ChapterDecision.REJECT),
    ):
        _chapter_table(chapters, height=400)
        with dpg.group(horizontal=True):
            for label, value in (
                ("Accept", ChapterDecision.ACCEPT),
                ("Edit", ChapterDecision.EDIT),
                ("Reject", ChapterDecision.REJECT),
            ):
                dpg.add_button(
                    label=label,
                    callback=_result_callback(tag, result, value),
                )


def chapter_edit_modal(
    chapters: list[Chapter],
    result: Queue[list[Chapter] | None],
) -> None:
    tag = dpg.generate_uuid()
    _OPEN_MODALS.add(tag)
    editor = dpg.generate_uuid()

    def save() -> None:
        _complete(tag, result, parse_chapters(dpg.get_value(editor)))

    with dpg.window(
        label="Edit chapters",
        tag=tag,
        modal=True,
        width=800,
        height=560,
        on_close=_result_callback(tag, result, None),
    ):
        dpg.add_input_text(
            tag=editor,
            default_value=serialize_chapters(chapters),
            multiline=True,
            width=-1,
            height=470,
        )
        with dpg.group(horizontal=True):
            dpg.add_button(label="Save", callback=save)
            dpg.add_button(
                label="Cancel",
                callback=_result_callback(tag, result, None),
            )


def beets_match_modal(
    task: BeetsTaskView,
    result: Queue[str | BeetsMatchDecision | None],
) -> None:
    tag = dpg.generate_uuid()
    _OPEN_MODALS.add(tag)
    with dpg.window(
        label="Beets matches",
        tag=tag,
        modal=True,
        width=850,
        height=500,
        on_close=_result_callback(tag, result, None),
    ):
        dpg.add_text(f"Task: {task.task_id}")
        with dpg.table(header_row=True, resizable=True, scrollY=True, height=390):
            for label in ("", "ID", "Artist", "Album", "Title", "Distance"):
                dpg.add_table_column(label=label)
            for match in task.matches:
                with dpg.table_row():
                    dpg.add_button(
                        label="Use",
                        callback=_result_callback(tag, result, match.candidate_id),
                    )
                    for value in (
                        match.candidate_id,
                        match.artist or "",
                        match.album or "",
                        match.title or "",
                        "" if match.distance is None else f"{match.distance:.3f}",
                    ):
                        dpg.add_text(value)
        with dpg.group(horizontal=True):
            dpg.add_button(
                label="As is",
                callback=_result_callback(tag, result, BeetsMatchDecision.AS_IS),
            )
            dpg.add_button(
                label="Skip",
                callback=_result_callback(tag, result, None),
            )


def duplicate_modal(
    duplicates: list[BeetsDuplicateView],
    result: Queue[BeetsDuplicateDecision],
) -> None:
    tag = dpg.generate_uuid()
    _OPEN_MODALS.add(tag)
    with dpg.window(
        label="Duplicates",
        tag=tag,
        modal=True,
        width=850,
        height=500,
        on_close=_result_callback(tag, result, BeetsDuplicateDecision.SKIP),
    ):
        with dpg.table(header_row=True, resizable=True, scrollY=True, height=390):
            for label in ("Artist", "Album", "Title", "Path"):
                dpg.add_table_column(label=label)
            for duplicate in duplicates:
                with dpg.table_row():
                    for value in (
                        duplicate.artist or "",
                        duplicate.album or "",
                        duplicate.title or "",
                        str(duplicate.path or ""),
                    ):
                        dpg.add_text(value)
        with dpg.group(horizontal=True):
            for label, value in (
                ("Skip", BeetsDuplicateDecision.SKIP),
                ("Keep all", BeetsDuplicateDecision.KEEP_ALL),
                ("Remove old", BeetsDuplicateDecision.REMOVE_OLD),
                ("Merge", BeetsDuplicateDecision.MERGE),
            ):
                dpg.add_button(
                    label=label,
                    callback=_result_callback(tag, result, value),
                )


def _chapter_table(chapters: list[Chapter], *, height: int) -> None:
    with dpg.table(header_row=True, resizable=True, scrollY=True, height=height):
        for label in ("#", "Start", "End", "Title", "Duration"):
            dpg.add_table_column(label=label)
        for chapter in chapters:
            with dpg.table_row():
                for value in (
                    str(chapter.index),
                    chapter.start_ts,
                    chapter.end_ts or "",
                    chapter.title,
                    chapter.duration_str,
                ):
                    dpg.add_text(value)


def _complete(tag: Any, result: Queue[Any], value: Any) -> None:
    try:
        result.put_nowait(value)
    except Full:
        pass
    if dpg.does_item_exist(tag):
        dpg.delete_item(tag)
    _OPEN_MODALS.discard(tag)


def _result_callback(tag: Any, result: Queue[Any], value: Any):
    def callback(
        sender: Any = None,
        app_data: Any = None,
        user_data: Any = None,
    ) -> None:
        _complete(tag, result, value)

    return callback


def close_all_modals() -> None:
    """Delete all open decision windows after their requests are cancelled."""
    for tag in tuple(_OPEN_MODALS):
        if dpg.does_item_exist(tag):
            dpg.delete_item(tag)
        _OPEN_MODALS.discard(tag)

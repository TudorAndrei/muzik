from pathlib import Path
from queue import Queue
from typing import cast

import dearpygui.dearpygui as dpg

from muzik.core.beets.decisions import BeetsMatchDecision
from muzik.core.beets.views import BeetsMatchView, BeetsTaskView
from muzik.gui.pipeline import (
    BEETS_DECISIONS,
    BEETS_MATCHES,
    BEETS_CURRENT_TAGS,
    BEETS_SOURCE,
    PIPELINE_OVERVIEW,
    PIPELINE_BUSY,
    PIPELINE_BUSY_TEXT,
    PIPELINE_ERROR,
    PIPELINE_WINDOW,
    STATUS,
    PipelineView,
)


def _descendants(parent: int | str) -> list[int | str]:
    result: list[int | str] = []
    pending = [parent]
    while pending:
        current = pending.pop()
        children = cast(dict[int, list[int | str]], dpg.get_item_children(current))
        for slot in children.values():
            result.extend(slot)
            pending.extend(slot)
    return result


def test_pipeline_shows_text_and_spinner_while_work_runs() -> None:
    dpg.create_context()
    view = PipelineView(lambda: None, lambda: None)
    try:
        view.build("Check playlists for new videos")

        assert dpg.does_item_exist(PIPELINE_BUSY)
        assert dpg.is_item_shown(PIPELINE_BUSY)
        assert dpg.get_value(PIPELINE_BUSY_TEXT) == "Working..."

        view.set_busy(False)

        assert not dpg.is_item_shown(PIPELINE_BUSY)

        view.show_error("Beets could not write to the library.")

        assert dpg.is_item_shown(PIPELINE_ERROR)
        assert dpg.get_value(PIPELINE_ERROR) == (
            "Beets could not write to the library."
        )
        assert dpg.get_value(STATUS) == "Workflow stopped."
    finally:
        view.destroy()
        dpg.destroy_context()


def test_pipeline_shows_beets_choices_and_returns_selection() -> None:
    dpg.create_context()
    view = PipelineView(lambda: None, lambda: None)
    result: Queue[str | BeetsMatchDecision | None] = Queue()
    task = BeetsTaskView(
        task_id="album",
        paths=[Path("/music/Hiromasa Suzuki - High-Flying/01 High-Flying.opus")],
        is_album=True,
        item_count=4,
        current_artist="Hiromasa Suzuki",
        current_album="High-Flying",
        current_year="1976",
        matches=[
            BeetsMatchView(
                candidate_id="first",
                artist="Hiromasa Suzuki",
                album="High-Flying",
                distance=0.08,
            ),
            BeetsMatchView(
                candidate_id="second",
                artist="Artist Two",
                album="Other Album",
                distance=0.54,
            ),
        ],
    )
    try:
        view.build("Organize album")
        view.request_beets_match(task, result)

        descendants = _descendants(PIPELINE_WINDOW)
        labels = [dpg.get_item_label(item) for item in descendants]
        assert labels.count("Use this match") == 2
        assert "Keep current tags" in labels
        assert "Skip this album" in labels
        assert dpg.is_item_shown(BEETS_DECISIONS)
        assert not dpg.is_item_shown(PIPELINE_OVERVIEW)
        assert dpg.is_item_shown(BEETS_MATCHES)
        assert dpg.get_value(BEETS_SOURCE).endswith(
            "Hiromasa Suzuki - High-Flying · 4 tracks"
        )
        assert dpg.get_value(BEETS_CURRENT_TAGS) == (
            "Hiromasa Suzuki — High-Flying · 1976"
        )
        assert dpg.get_value(STATUS) == "Choose an album match."

        text_values = [
            dpg.get_value(item)
            for item in descendants
            if dpg.get_item_type(item) == "mvAppItemType::mvText"
        ]
        assert "Hiromasa Suzuki — High-Flying" in text_values
        assert "Good match · Difference 0.080" in text_values
        assert "Low confidence · Difference 0.540" in text_values

        use_button = next(
            item for item in descendants if dpg.get_item_label(item) == "Use this match"
        )
        callback = dpg.get_item_callback(use_button)
        assert callback is not None
        dpg.run_callbacks([[callback, use_button, None, None]])

        assert result.get_nowait() in {"first", "second"}
        assert not dpg.is_item_shown(BEETS_DECISIONS)
        assert dpg.is_item_shown(PIPELINE_OVERVIEW)
    finally:
        view.destroy()
        dpg.destroy_context()

from queue import Queue
from typing import cast

import dearpygui.dearpygui as dpg

from muzik.core.beets.decisions import BeetsMatchDecision
from muzik.core.beets.views import BeetsMatchView, BeetsTaskView
from muzik.gui.pipeline import (
    BEETS_DECISIONS,
    PIPELINE_BUSY,
    PIPELINE_BUSY_TEXT,
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
    finally:
        view.destroy()
        dpg.destroy_context()


def test_pipeline_shows_beets_choices_and_returns_selection() -> None:
    dpg.create_context()
    view = PipelineView(lambda: None, lambda: None)
    result: Queue[str | BeetsMatchDecision | None] = Queue()
    task = BeetsTaskView(
        task_id="album",
        matches=[
            BeetsMatchView(candidate_id="first", artist="Artist One"),
            BeetsMatchView(candidate_id="second", artist="Artist Two"),
        ],
    )
    try:
        view.build("Organize album")
        view.request_beets_match(task, result)

        descendants = _descendants(PIPELINE_WINDOW)
        labels = [dpg.get_item_label(item) for item in descendants]
        assert labels.count("Use") == 2
        assert "Import as is" in labels
        assert "Skip" in labels
        assert dpg.is_item_shown(BEETS_DECISIONS)
        assert dpg.get_value(STATUS) == "Choose a Beets match."

        use_button = next(
            item for item in descendants if dpg.get_item_label(item) == "Use"
        )
        callback = dpg.get_item_callback(use_button)
        assert callback is not None
        dpg.run_callbacks([[callback, use_button, None, None]])

        assert result.get_nowait() in {"first", "second"}
        assert not dpg.is_item_shown(BEETS_DECISIONS)
    finally:
        view.destroy()
        dpg.destroy_context()

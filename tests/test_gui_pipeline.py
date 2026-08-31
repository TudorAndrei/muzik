import dearpygui.dearpygui as dpg

from muzik.gui.pipeline import PIPELINE_BUSY, PIPELINE_BUSY_TEXT, PipelineView


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

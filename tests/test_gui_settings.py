import dearpygui.dearpygui as dpg

from muzik.core.services import ServiceStatus
from muzik.gui.settings import SETTINGS_STATUS, SETTINGS_TABLE, SettingsView


def _row_count() -> int:
    return len(dpg.get_item_children(SETTINGS_TABLE, slot=1) or [])


def test_settings_loads_statuses_and_summarizes() -> None:
    dpg.create_context()
    view = SettingsView(lambda: None, lambda: None)
    try:
        view.build()
        view.load_statuses(
            [
                ServiceStatus("ffmpeg", True, "ffmpeg version 9.0"),
                ServiceStatus("yt-dlp", False, "Not found on PATH (install yt-dlp)."),
                ServiceStatus(
                    "slskd (Soulseek)", None, "Not configured.", optional=True
                ),
            ]
        )

        assert _row_count() == 3
        assert dpg.get_value(SETTINGS_STATUS) == "1 service(s) unavailable."
    finally:
        dpg.destroy_context()


def test_settings_reports_all_available() -> None:
    dpg.create_context()
    view = SettingsView(lambda: None, lambda: None)
    try:
        view.build()
        view.load_statuses([ServiceStatus("ffmpeg", True, "v9")])

        assert dpg.get_value(SETTINGS_STATUS) == "All required services are available."
    finally:
        dpg.destroy_context()


def test_load_statuses_is_noop_without_window() -> None:
    dpg.create_context()
    view = SettingsView(lambda: None, lambda: None)
    try:
        # No build(): the table does not exist, so this must not raise.
        view.load_statuses([ServiceStatus("ffmpeg", True, "v9")])
    finally:
        dpg.destroy_context()

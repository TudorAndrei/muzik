"""Settings window that reports external service availability."""

from __future__ import annotations

from collections.abc import Callable, Iterable

import dearpygui.dearpygui as dpg

from muzik.core.services import ServiceStatus


SETTINGS_WINDOW = "settings-window"
SETTINGS_STATUS = "settings-status"
SETTINGS_TABLE = "settings-table"
SETTINGS_RECHECK = "settings-recheck"

_OK_COLOR = (120, 200, 120)
_FAIL_COLOR = (230, 120, 120)
_NA_COLOR = (190, 190, 120)


class SettingsView:
    """Own the render-thread widgets for the service-availability page."""

    def __init__(
        self,
        on_recheck: Callable[..., None],
        on_close: Callable[..., None],
    ) -> None:
        self._on_recheck = on_recheck
        self._on_close = on_close

    def build(self) -> None:
        with dpg.window(
            tag=SETTINGS_WINDOW,
            label="Settings - service availability",
            modal=True,
            width=760,
            height=460,
            on_close=self._on_close,
        ):
            dpg.add_text("Service availability", color=(100, 180, 255))
            dpg.add_text("Checking...", tag=SETTINGS_STATUS)
            with dpg.table(
                tag=SETTINGS_TABLE,
                header_row=True,
                resizable=True,
                policy=dpg.mvTable_SizingStretchProp,
                scrollY=True,
                height=330,
            ):
                dpg.add_table_column(label="Service")
                dpg.add_table_column(label="Status")
                dpg.add_table_column(label="Detail")
            with dpg.group(horizontal=True):
                dpg.add_button(
                    label="Re-check",
                    tag=SETTINGS_RECHECK,
                    callback=self._on_recheck,
                    width=100,
                )
                dpg.add_button(label="Close", callback=self._on_close, width=100)

    def destroy(self) -> None:
        if dpg.does_item_exist(SETTINGS_WINDOW):
            dpg.delete_item(SETTINGS_WINDOW)

    def set_checking(self) -> None:
        if not dpg.does_item_exist(SETTINGS_TABLE):
            return
        dpg.set_value(SETTINGS_STATUS, "Checking...")
        dpg.disable_item(SETTINGS_RECHECK)
        dpg.delete_item(SETTINGS_TABLE, children_only=True, slot=1)

    def load_statuses(self, statuses: Iterable[ServiceStatus]) -> None:
        if not dpg.does_item_exist(SETTINGS_TABLE):
            return
        rows = list(statuses)
        dpg.delete_item(SETTINGS_TABLE, children_only=True, slot=1)
        for status in rows:
            with dpg.table_row(parent=SETTINGS_TABLE):
                dpg.add_text(status.name)
                label, color = _status_cell(status)
                dpg.add_text(label, color=color)
                dpg.add_text(status.detail)
        failed = sum(1 for status in rows if status.available is False)
        if failed:
            dpg.set_value(SETTINGS_STATUS, f"{failed} service(s) unavailable.")
        else:
            dpg.set_value(SETTINGS_STATUS, "All required services are available.")
        if dpg.does_item_exist(SETTINGS_RECHECK):
            dpg.enable_item(SETTINGS_RECHECK)


def _status_cell(status: ServiceStatus) -> tuple[str, tuple[int, int, int]]:
    if status.available is True:
        return "OK", _OK_COLOR
    if status.available is None:
        return "not configured", _NA_COLOR
    return ("optional: unavailable" if status.optional else "MISSING"), _FAIL_COLOR

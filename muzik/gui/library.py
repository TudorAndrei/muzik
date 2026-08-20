"""Library window that lists audio already in the output folder."""

from __future__ import annotations

import datetime
from collections.abc import Callable, Iterable
from pathlib import Path

import dearpygui.dearpygui as dpg

from muzik.core.library import DownloadedItem, human_size
from muzik.gui.theme import ACCENT


LIBRARY_WINDOW = "library-window"
LIBRARY_STATUS = "library-status"
LIBRARY_TABLE = "library-table"
LIBRARY_REFRESH = "library-refresh"


class LibraryView:
    """Own the render-thread widgets for the downloaded-audio inventory."""

    def __init__(
        self,
        output: Path,
        on_refresh: Callable[..., None],
        on_close: Callable[..., None],
    ) -> None:
        self._output = output
        self._on_refresh = on_refresh
        self._on_close = on_close

    def build(self) -> None:
        with dpg.window(
            tag=LIBRARY_WINDOW,
            label="Library - downloaded audio",
            on_close=self._on_close,
        ):
            dpg.add_text("Downloaded audio", color=ACCENT)
            dpg.add_text(str(self._output), color=(150, 150, 158))
            dpg.add_text("Scanning...", tag=LIBRARY_STATUS)
            with dpg.table(
                tag=LIBRARY_TABLE,
                header_row=True,
                resizable=True,
                policy=dpg.mvTable_SizingStretchProp,
                scrollY=True,
                height=-40,
            ):
                dpg.add_table_column(label="Title")
                dpg.add_table_column(
                    label="YouTube ID", width_fixed=True, init_width_or_weight=110
                )
                dpg.add_table_column(
                    label="Format", width_fixed=True, init_width_or_weight=70
                )
                dpg.add_table_column(
                    label="Size", width_fixed=True, init_width_or_weight=80
                )
                dpg.add_table_column(
                    label="Modified", width_fixed=True, init_width_or_weight=130
                )
            with dpg.group(horizontal=True):
                dpg.add_button(
                    label="Refresh",
                    tag=LIBRARY_REFRESH,
                    callback=self._on_refresh,
                    width=100,
                )
                dpg.add_button(label="Back", callback=self._on_close, width=100)

    def destroy(self) -> None:
        if dpg.does_item_exist(LIBRARY_WINDOW):
            dpg.delete_item(LIBRARY_WINDOW)

    def set_scanning(self) -> None:
        if not dpg.does_item_exist(LIBRARY_TABLE):
            return
        dpg.set_value(LIBRARY_STATUS, "Scanning...")
        dpg.disable_item(LIBRARY_REFRESH)
        dpg.delete_item(LIBRARY_TABLE, children_only=True, slot=1)

    def load_items(self, items: Iterable[DownloadedItem]) -> None:
        if not dpg.does_item_exist(LIBRARY_TABLE):
            return
        rows = list(items)
        dpg.delete_item(LIBRARY_TABLE, children_only=True, slot=1)
        total = 0
        for item in rows:
            total += item.size
            with dpg.table_row(parent=LIBRARY_TABLE):
                dpg.add_text(item.title)
                dpg.add_text(item.youtube_id or "—")
                dpg.add_text(item.ext)
                dpg.add_text(human_size(item.size))
                dpg.add_text(_modified(item.mtime))
        if rows:
            dpg.set_value(
                LIBRARY_STATUS,
                f"{len(rows)} file(s), {human_size(total)}.",
            )
        else:
            dpg.set_value(LIBRARY_STATUS, "No downloads found.")
        if dpg.does_item_exist(LIBRARY_REFRESH):
            dpg.enable_item(LIBRARY_REFRESH)


def _modified(mtime: float) -> str:
    return datetime.datetime.fromtimestamp(mtime).strftime("%Y-%m-%d %H:%M")

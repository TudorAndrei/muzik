"""Top-level shell: one primary window with a real navigation tab bar.

Workflow/Watchlist/Library/Settings are built once, as real `dpg.tab`
children of one `dpg.tab_bar` — switching between them is native DearPyGui
behavior (instant, no rebuild). This replaces the previous pattern where
each page was its own top-level `dpg.window`, shown/hidden and repeatedly
fought over `dpg.set_primary_window`.
"""

from __future__ import annotations

from collections.abc import Callable
from typing import Any

import dearpygui.dearpygui as dpg

from muzik.branding import logo_path
from muzik.gui.launcher import LauncherView
from muzik.gui.library import LibraryView
from muzik.gui.settings import SettingsView
from muzik.gui.watchlist import WatchlistView


MAIN_WINDOW = "main-window"
NAV_TABS = "main-nav-tabs"
TAB_WORKFLOW = "main-tab-workflow"
TAB_WATCHLIST = "main-tab-watchlist"
TAB_LIBRARY = "main-tab-library"
TAB_SETTINGS = "main-tab-settings"

LOGO_TEXTURE_REGISTRY = "main-logo-texture-registry"
LOGO_TEXTURE = "main-logo-texture"
LOGO_IMAGE = "main-logo"


def build(
    *,
    launcher: LauncherView,
    watchlist: WatchlistView,
    library: LibraryView,
    settings: SettingsView,
    on_tab_changed: Callable[[str], None],
    on_quit: Callable[..., None],
) -> None:
    """Build the single primary window and its real navigation tab bar."""
    has_logo = _build_logo_texture()
    with dpg.window(tag=MAIN_WINDOW):
        # A plain horizontal group can't reserve space for the trailing Quit
        # button: the tab bar has no fixed width, so its rendered width (and
        # therefore where Quit lands) isn't stable across frames/resizes. A
        # two-column table reserves Quit's column up front instead.
        with dpg.table(
            header_row=False,
            policy=dpg.mvTable_SizingStretchProp,
            borders_innerV=False,
            borders_outerH=False,
            borders_outerV=False,
            borders_innerH=False,
            no_pad_outerX=True,
        ):
            dpg.add_table_column(width_stretch=True)
            dpg.add_table_column(width_fixed=True, init_width_or_weight=90)
            with dpg.table_row():
                with dpg.table_cell():
                    with dpg.group(horizontal=True):
                        if has_logo:
                            dpg.add_image(
                                LOGO_TEXTURE, tag=LOGO_IMAGE, width=32, height=32
                            )
                        with dpg.tab_bar(
                            tag=NAV_TABS, callback=_dispatch(on_tab_changed)
                        ):
                            with dpg.tab(label="Workflow", tag=TAB_WORKFLOW):
                                launcher.build(parent=TAB_WORKFLOW)
                            with dpg.tab(label="Watchlist", tag=TAB_WATCHLIST):
                                watchlist.build(parent=TAB_WATCHLIST)
                            with dpg.tab(label="Library", tag=TAB_LIBRARY):
                                library.build(parent=TAB_LIBRARY)
                            with dpg.tab(label="Settings", tag=TAB_SETTINGS):
                                settings.build(parent=TAB_SETTINGS)
                with dpg.table_cell():
                    dpg.add_button(label="Quit", callback=on_quit, width=80)
    dpg.set_primary_window(MAIN_WINDOW, True)


def _dispatch(on_tab_changed: Callable[[str], None]) -> Callable[..., None]:
    def callback(
        sender: Any = None,
        app_data: Any = None,
        user_data: Any = None,
    ) -> None:
        tab = app_data if isinstance(app_data, str) else dpg.get_value(NAV_TABS)
        on_tab_changed(str(tab))

    return callback


def _build_logo_texture() -> bool:
    if dpg.does_item_exist(LOGO_TEXTURE):
        return True
    path = logo_path()
    if path is None:
        return False
    try:
        width, height, _channels, data = dpg.load_image(str(path))
    except OSError, RuntimeError, SystemError, ValueError:
        return False
    with dpg.texture_registry(tag=LOGO_TEXTURE_REGISTRY):
        dpg.add_static_texture(width, height, data, tag=LOGO_TEXTURE)
    return True

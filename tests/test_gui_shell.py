from pathlib import Path
from typing import cast

import dearpygui.dearpygui as dpg

from muzik.gui.launcher import FIELD_TAGS, LauncherView
from muzik.gui.library import LIBRARY_TABLE, LIBRARY_ROOT, LibraryView
from muzik.gui.settings import SETTINGS_TABLE, SETTINGS_ROOT, SettingsView
from muzik.gui.shell import MAIN_WINDOW, NAV_TABS, TAB_WATCHLIST, build
from muzik.gui.watchlist import GRID, WATCHLIST_ROOT, WatchlistView


def _find_item_by_label(root: int | str, label: str) -> int | str | None:
    pending: list[int | str] = [root]
    while pending:
        item = pending.pop()
        if dpg.get_item_label(item) == label:
            return item
        children = cast(dict[int, list[int]], dpg.get_item_children(item))
        for slot in children.values():
            pending.extend(slot)
    return None


def _has_ancestor(item: int | str, ancestor: int | str) -> bool:
    parent = dpg.get_item_parent(item)
    while parent:
        if parent == ancestor:
            return True
        parent = dpg.get_item_parent(parent)
    return False


def _build_shell(*, on_tab_changed=lambda tab: None, on_quit=lambda: None) -> None:
    build(
        launcher=LauncherView(lambda config: None, lambda: None),
        watchlist=WatchlistView(
            on_add=lambda url: None,
            on_remove=lambda playlist_id: None,
            on_refresh=lambda: None,
            on_action=lambda playlist_id, item, action: None,
            on_back=lambda: None,
            on_quit=lambda: None,
        ),
        library=LibraryView(Path("."), lambda: None, lambda: None),
        settings=SettingsView(lambda: None, lambda: None),
        on_tab_changed=on_tab_changed,
        on_quit=on_quit,
    )


def test_shell_builds_one_window_with_four_real_tabs() -> None:
    dpg.create_context()
    try:
        _build_shell()

        assert dpg.does_item_exist(MAIN_WINDOW)
        assert dpg.does_item_exist(NAV_TABS)
        for label in ("Workflow", "Watchlist", "Library", "Settings"):
            item = _find_item_by_label(MAIN_WINDOW, label)
            assert item is not None
            assert _has_ancestor(item, NAV_TABS)

        # Each page's real content lives inside the shared tab bar/window —
        # never as its own separate top-level window (the reported bug).
        for tag in (FIELD_TAGS["raw"], GRID, LIBRARY_TABLE, SETTINGS_TABLE):
            assert dpg.does_item_exist(tag)
            assert _has_ancestor(tag, MAIN_WINDOW)
        for root in (WATCHLIST_ROOT, LIBRARY_ROOT, SETTINGS_ROOT):
            assert dpg.get_item_type(root) != "mvAppItemType::mvWindowAppItem"
    finally:
        dpg.destroy_context()


def test_tab_bar_callback_reports_the_newly_selected_tab() -> None:
    seen: list[str] = []
    dpg.create_context()
    try:
        _build_shell(on_tab_changed=seen.append)

        dpg.set_value(NAV_TABS, TAB_WATCHLIST)
        callback = dpg.get_item_callback(NAV_TABS)
        assert callback is not None
        callback(NAV_TABS, TAB_WATCHLIST, None)

        assert seen == [TAB_WATCHLIST]
    finally:
        dpg.destroy_context()

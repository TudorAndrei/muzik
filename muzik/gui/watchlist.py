"""YouTube-style playlist watchlist page for the desktop interface."""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass
from math import ceil
from pathlib import Path
from typing import Any

import dearpygui.dearpygui as dpg

from muzik.core.watchlist import (
    STAGE_NAMES,
    StageStatus,
    Watchlist,
    WatchlistItem,
    WatchlistPlaylist,
)
from muzik.core.workflow.item_actions import (
    ItemAction,
    item_action_availability,
    item_summary_state,
    primary_item_action,
)
from muzik.core.workflow.service import WorkflowRequest
from muzik.gui.theme import (
    ACCENT,
    FAIL_COLOR,
    OK_COLOR,
    STAGE_COMPLETE,
    STAGE_FAILED,
    STAGE_NOT_STARTED,
    STAGE_RUNNING,
    STAGE_SKIPPED,
    STAGE_STALE,
    bind_primary_button,
)


WATCHLIST_WINDOW = "watchlist-window"
PLAYLIST_RAIL = "watchlist-playlist-rail"
GRID = "watchlist-grid"
ADD_URL = "watchlist-add-url"
ERROR_TEXT = "watchlist-error"
EMPTY_TEXT = "watchlist-empty"
FILTER = "watchlist-filter"
PAGE_TEXT = "watchlist-page-text"
PREVIOUS_BUTTON = "watchlist-previous"
NEXT_BUTTON = "watchlist-next"
REMOVE_BUTTON = "watchlist-remove"
REFRESH_BUTTON = "watchlist-refresh"
TEXTURE_REGISTRY = "watchlist-textures"
ACTIONS_WINDOW = "watchlist-actions"

FILTERS = ("All", "Pending", "Processing", "Failed", "Processed", "Unavailable")
PAGE_SIZE = 6
ACTION_LABELS = {
    ItemAction.RUN: "Run",
    ItemAction.RETRY: "Retry",
    ItemAction.DOWNLOAD_AGAIN: "Download again",
    ItemAction.PARSE_AGAIN: "Parse again",
    ItemAction.SPLIT_AGAIN: "Split again",
    ItemAction.ORGANIZE_AGAIN: "Organize again",
    ItemAction.RUN_ALL_AGAIN: "Run all again",
}

_STAGE_LABELS = {
    "download": "Download",
    "parse": "Parse",
    "split": "Split",
    "organize": "Organize",
}
_STATUS_LABELS = {
    StageStatus.NOT_STARTED: "Not started",
    StageStatus.RUNNING: "Running",
    StageStatus.COMPLETE: "Complete",
    StageStatus.FAILED: "Failed",
    StageStatus.SKIPPED: "Skipped",
    StageStatus.STALE: "Stale",
}
_STATUS_COLORS = {
    StageStatus.NOT_STARTED: STAGE_NOT_STARTED,
    StageStatus.RUNNING: STAGE_RUNNING,
    StageStatus.COMPLETE: STAGE_COMPLETE,
    StageStatus.FAILED: STAGE_FAILED,
    StageStatus.SKIPPED: STAGE_SKIPPED,
    StageStatus.STALE: STAGE_STALE,
}


@dataclass(frozen=True, slots=True)
class WatchlistPage:
    items: tuple[WatchlistItem, ...]
    page: int
    page_count: int


def columns_for_width(width: int) -> int:
    """Return a compact card column count for the available main width."""
    return max(1, min(4, width // 310))


def page_for_items(
    items: list[WatchlistItem],
    *,
    status_filter: str,
    page: int,
    page_size: int = PAGE_SIZE,
) -> WatchlistPage:
    """Filter and page items without DearPyGui state."""
    filtered = [
        item
        for item in items
        if status_filter == "All" or item_summary_state(item) == status_filter
    ]
    page_count = max(1, ceil(len(filtered) / page_size))
    safe_page = min(max(page, 0), page_count - 1)
    start = safe_page * page_size
    return WatchlistPage(
        tuple(filtered[start : start + page_size]), safe_page, page_count
    )


class WatchlistView:
    """Own the render-thread widgets and textures for the watchlist page."""

    def __init__(
        self,
        *,
        on_add: Callable[[str], None],
        on_remove: Callable[[str], None],
        on_refresh: Callable[..., None],
        on_action: Callable[[str, WatchlistItem, ItemAction], None],
        on_back: Callable[..., None],
        on_quit: Callable[..., None],
    ) -> None:
        self._on_add = on_add
        self._on_remove = on_remove
        self._on_refresh = on_refresh
        self._on_action = on_action
        self._on_back = on_back
        self._on_quit = on_quit
        self._watchlist = Watchlist()
        self._request: WorkflowRequest | None = None
        self._selected_playlist_id: str | None = None
        self._filter = "All"
        self._page = 0
        self._columns = 3
        self._textures: dict[str, Any] = {}

    def build(self) -> None:
        with dpg.window(
            tag=WATCHLIST_WINDOW,
            label="Playlist watchlist",
            on_close=self._on_back,
        ):
            with dpg.group(horizontal=True):
                dpg.add_input_text(
                    tag=ADD_URL,
                    hint="YouTube playlist URL",
                    width=-510,
                    on_enter=True,
                    callback=self._add,
                )
                dpg.add_button(label="Add playlist", callback=self._add, width=120)
                refresh = dpg.add_button(
                    label="Refresh new videos",
                    tag=REFRESH_BUTTON,
                    callback=self._on_refresh,
                    width=150,
                )
                dpg.add_button(label="Back", callback=self._on_back, width=80)
                dpg.add_button(label="Quit", callback=self._on_quit, width=80)
                bind_primary_button(refresh)
            dpg.add_text("", tag=ERROR_TEXT, color=FAIL_COLOR)
            with dpg.group(horizontal=True):
                with dpg.child_window(tag=PLAYLIST_RAIL, width=250, border=True):
                    dpg.add_text("Playlists", color=ACCENT)
                with dpg.child_window(width=-1, height=-1, border=False):
                    with dpg.group(horizontal=True):
                        dpg.add_combo(
                            FILTERS,
                            default_value="All",
                            tag=FILTER,
                            label="Status",
                            callback=self._filter_changed,
                            width=150,
                        )
                        dpg.add_button(
                            label="Previous",
                            tag=PREVIOUS_BUTTON,
                            callback=self._previous_page,
                            width=100,
                        )
                        dpg.add_text("Page 1 of 1", tag=PAGE_TEXT)
                        dpg.add_button(
                            label="Next",
                            tag=NEXT_BUTTON,
                            callback=self._next_page,
                            width=100,
                        )
                        dpg.add_button(
                            label="Remove playlist",
                            tag=REMOVE_BUTTON,
                            callback=self._remove,
                            width=140,
                        )
                    dpg.add_text("", tag=EMPTY_TEXT)
                    with dpg.child_window(tag=GRID, width=-1, height=-1, border=False):
                        pass
        with dpg.texture_registry(tag=TEXTURE_REGISTRY):
            pass
        dpg.set_primary_window(WATCHLIST_WINDOW, True)
        self._render()

    def load(self, watchlist: Watchlist, request: WorkflowRequest) -> None:
        self._watchlist = watchlist
        self._request = request
        playlist_ids = {playlist.playlist_id for playlist in watchlist.playlists}
        if self._selected_playlist_id not in playlist_ids:
            self._selected_playlist_id = (
                watchlist.playlists[0].playlist_id if watchlist.playlists else None
            )
        self._page = 0
        self._render()

    def show_error(self, message: str) -> None:
        dpg.set_value(ERROR_TEXT, message)

    def clear_error(self) -> None:
        dpg.set_value(ERROR_TEXT, "")

    def set_busy(self, busy: bool) -> None:
        if busy:
            dpg.disable_item(REFRESH_BUTTON)
        else:
            dpg.enable_item(REFRESH_BUTTON)

    def set_available_width(self, width: int) -> None:
        columns = columns_for_width(width)
        if columns != self._columns:
            self._columns = columns
            self._render_cards()

    def load_cached_thumbnail(self, video_id: str, path: Path) -> bool:
        """Create one texture. Call this method only on the render thread."""
        if video_id in self._textures or not path.is_file():
            return video_id in self._textures
        width, height, _channels, data = dpg.load_image(str(path))
        tag = dpg.generate_uuid()
        dpg.add_static_texture(
            width=width,
            height=height,
            default_value=data,
            tag=tag,
            parent=TEXTURE_REGISTRY,
        )
        self._textures[video_id] = tag
        self._render_cards()
        return True

    def release_textures(self) -> None:
        for tag in tuple(self._textures.values()):
            if dpg.does_item_exist(tag):
                dpg.delete_item(tag)
        self._textures.clear()

    def show(self) -> None:
        dpg.show_item(WATCHLIST_WINDOW)
        dpg.set_primary_window(WATCHLIST_WINDOW, True)

    def hide(self) -> None:
        dpg.hide_item(WATCHLIST_WINDOW)

    def destroy(self) -> None:
        self.release_textures()
        if dpg.does_item_exist(TEXTURE_REGISTRY):
            dpg.delete_item(TEXTURE_REGISTRY)
        if dpg.does_item_exist(ACTIONS_WINDOW):
            dpg.delete_item(ACTIONS_WINDOW)
        if dpg.does_item_exist(WATCHLIST_WINDOW):
            dpg.delete_item(WATCHLIST_WINDOW)

    def _selected_playlist(self) -> WatchlistPlaylist | None:
        return next(
            (
                playlist
                for playlist in self._watchlist.playlists
                if playlist.playlist_id == self._selected_playlist_id
            ),
            None,
        )

    def _render(self) -> None:
        if not dpg.does_item_exist(WATCHLIST_WINDOW):
            return
        self._render_playlists()
        self._render_cards()

    def _render_playlists(self) -> None:
        dpg.delete_item(PLAYLIST_RAIL, children_only=True)
        dpg.add_text("Playlists", color=ACCENT, parent=PLAYLIST_RAIL)
        if not self._watchlist.playlists:
            dpg.add_text("Add a playlist to start.", wrap=220, parent=PLAYLIST_RAIL)
            return
        for playlist in self._watchlist.playlists:
            selected = playlist.playlist_id == self._selected_playlist_id
            dpg.add_selectable(
                label=playlist.playlist_id,
                default_value=selected,
                callback=self._select_playlist,
                user_data=playlist.playlist_id,
                parent=PLAYLIST_RAIL,
                width=-1,
            )
            dpg.add_text(
                f"{len(playlist.items)} video(s)",
                color=OK_COLOR if not playlist.last_error else FAIL_COLOR,
                parent=PLAYLIST_RAIL,
            )
            if playlist.last_checked_at:
                dpg.add_text(
                    f"Checked: {playlist.last_checked_at}",
                    wrap=220,
                    parent=PLAYLIST_RAIL,
                )
            if playlist.last_error:
                dpg.add_text(
                    playlist.last_error,
                    color=FAIL_COLOR,
                    wrap=220,
                    parent=PLAYLIST_RAIL,
                )
            dpg.add_separator(parent=PLAYLIST_RAIL)

    def _render_cards(self) -> None:
        if not dpg.does_item_exist(GRID):
            return
        dpg.delete_item(GRID, children_only=True)
        playlist = self._selected_playlist()
        dpg.configure_item(REMOVE_BUTTON, enabled=playlist is not None)
        if playlist is None:
            dpg.set_value(EMPTY_TEXT, "No playlist is selected.")
            self._set_page_controls(0, 1)
            return
        page = page_for_items(
            playlist.items,
            status_filter=self._filter,
            page=self._page,
        )
        self._page = page.page
        self._set_page_controls(page.page, page.page_count)
        if not page.items:
            dpg.set_value(EMPTY_TEXT, "No videos match this status.")
            return
        dpg.set_value(EMPTY_TEXT, "")
        with dpg.table(
            parent=GRID,
            header_row=False,
            borders_innerV=False,
            policy=dpg.mvTable_SizingStretchSame,
        ):
            for _ in range(self._columns):
                dpg.add_table_column()
            for start in range(0, len(page.items), self._columns):
                with dpg.table_row():
                    row = page.items[start : start + self._columns]
                    for item in row:
                        self._add_card(playlist.playlist_id, item)
                    for _ in range(self._columns - len(row)):
                        dpg.add_text("")

    def _add_card(self, playlist_id: str, item: WatchlistItem) -> None:
        with dpg.table_cell():
            with dpg.child_window(height=390, border=True):
                texture = self._textures.get(item.video_id or "")
                if texture is not None:
                    dpg.add_image(texture, width=-1, height=150)
                else:
                    with dpg.child_window(height=150, border=False):
                        dpg.add_spacer(height=50)
                        dpg.add_text("No cached thumbnail", color=STAGE_NOT_STARTED)
                dpg.add_text(f"{item.position}. {item.title}", wrap=-1)
                dpg.add_text(
                    f"YouTube ID: {item.video_id or 'Unavailable'}",
                    color=STAGE_NOT_STARTED,
                )
                summary = item_summary_state(item)
                summary_color = {
                    "Processed": OK_COLOR,
                    "Failed": FAIL_COLOR,
                    "Processing": ACCENT,
                }.get(summary, STAGE_NOT_STARTED)
                dpg.add_text(f"State: {summary}", color=summary_color)
                self._add_stage_rail(item)
                with dpg.group(horizontal=True):
                    action, label = primary_item_action(item)
                    primary = dpg.add_button(
                        label=label,
                        callback=lambda s=None, a=None, u=None, selected=action: (
                            None
                            if selected is None
                            else self._on_action(playlist_id, item, selected)
                        ),
                        width=110,
                        height=40,
                        enabled=action is not None,
                    )
                    if action is not None:
                        bind_primary_button(primary)
                    dpg.add_button(
                        label="Actions...",
                        callback=lambda s=None, a=None, u=None: self._open_actions(
                            playlist_id, item
                        ),
                        width=110,
                        height=40,
                        enabled=item.video_id is not None,
                    )
                if item.last_error:
                    dpg.add_text(item.last_error, color=FAIL_COLOR, wrap=-1)

    def _add_stage_rail(self, item: WatchlistItem) -> None:
        with dpg.group(horizontal=True):
            for name in STAGE_NAMES:
                record = item.stages[name]
                with dpg.child_window(width=72, height=57, border=True):
                    dpg.add_text(_STAGE_LABELS[name])
                    dpg.add_text(
                        _STATUS_LABELS[record.status],
                        color=_STATUS_COLORS[record.status],
                    )

    def _open_actions(self, playlist_id: str, item: WatchlistItem) -> None:
        if dpg.does_item_exist(ACTIONS_WINDOW):
            dpg.delete_item(ACTIONS_WINDOW)
        with dpg.window(
            tag=ACTIONS_WINDOW,
            label=f"Commands - {item.title}",
            modal=True,
            width=560,
            height=520,
            on_close=self._close_actions,
        ):
            dpg.add_text(item.title, wrap=520)
            dpg.add_text(f"YouTube ID: {item.video_id or 'Unavailable'}")
            if self._request is None:
                dpg.add_text("Launcher paths are not available.", color=FAIL_COLOR)
                return
            for action, label in ACTION_LABELS.items():
                available = item_action_availability(
                    item,
                    action,
                    request=self._request,
                )
                dpg.add_button(
                    label=label,
                    width=180,
                    height=36,
                    enabled=available.enabled,
                    callback=lambda s=None, a=None, u=None, selected=action: (
                        self._choose_action(playlist_id, item, selected)
                    ),
                )
                if not available.enabled and available.reason:
                    dpg.add_text(
                        available.reason,
                        color=STAGE_NOT_STARTED,
                        wrap=500,
                    )
            dpg.add_button(label="Close", callback=self._close_actions, width=100)

    def _choose_action(
        self,
        playlist_id: str,
        item: WatchlistItem,
        action: ItemAction,
    ) -> None:
        self._close_actions()
        self._on_action(playlist_id, item, action)

    def _close_actions(
        self,
        sender: Any = None,
        app_data: Any = None,
        user_data: Any = None,
    ) -> None:
        if dpg.does_item_exist(ACTIONS_WINDOW):
            dpg.delete_item(ACTIONS_WINDOW)

    def _select_playlist(
        self,
        sender: Any = None,
        app_data: Any = None,
        user_data: Any = None,
    ) -> None:
        self._selected_playlist_id = str(user_data)
        self._page = 0
        self._render()

    def _filter_changed(
        self,
        sender: Any = None,
        app_data: Any = None,
        user_data: Any = None,
    ) -> None:
        self._filter = str(app_data or dpg.get_value(FILTER))
        self._page = 0
        self._render_cards()

    def _previous_page(
        self,
        sender: Any = None,
        app_data: Any = None,
        user_data: Any = None,
    ) -> None:
        self._page -= 1
        self._render_cards()

    def _next_page(
        self,
        sender: Any = None,
        app_data: Any = None,
        user_data: Any = None,
    ) -> None:
        self._page += 1
        self._render_cards()

    def _set_page_controls(self, page: int, page_count: int) -> None:
        dpg.set_value(PAGE_TEXT, f"Page {page + 1} of {page_count}")
        dpg.configure_item(PREVIOUS_BUTTON, enabled=page > 0)
        dpg.configure_item(NEXT_BUTTON, enabled=page + 1 < page_count)

    def _add(
        self,
        sender: Any = None,
        app_data: Any = None,
        user_data: Any = None,
    ) -> None:
        value = str(dpg.get_value(ADD_URL)).strip()
        if not value:
            self.show_error("Enter a YouTube playlist URL.")
            return
        self._on_add(value)

    def _remove(
        self,
        sender: Any = None,
        app_data: Any = None,
        user_data: Any = None,
    ) -> None:
        if self._selected_playlist_id is not None:
            self._on_remove(self._selected_playlist_id)

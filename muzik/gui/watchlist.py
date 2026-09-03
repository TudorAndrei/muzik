"""YouTube-style playlist watchlist page for the desktop interface."""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass
from functools import partial
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
    NA_COLOR,
    OK_COLOR,
    STAGE_COMPLETE,
    STAGE_FAILED,
    STAGE_NOT_STARTED,
    STAGE_RUNNING,
    STAGE_SKIPPED,
    STAGE_STALE,
    bind_primary_button,
)


WATCHLIST_ROOT = "watchlist-root"
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
    ItemAction.CHECK_QUALITY_AGAIN: "Check quality again",
    ItemAction.PARSE_AGAIN: "Parse again",
    ItemAction.SPLIT_AGAIN: "Split again",
    ItemAction.ORGANIZE_AGAIN: "Organize again",
    ItemAction.RUN_ALL_AGAIN: "Run all again",
}
_REPEAT_ACTIONS = (
    ItemAction.DOWNLOAD_AGAIN,
    ItemAction.CHECK_QUALITY_AGAIN,
    ItemAction.PARSE_AGAIN,
    ItemAction.SPLIT_AGAIN,
    ItemAction.ORGANIZE_AGAIN,
    ItemAction.RUN_ALL_AGAIN,
)

_STAGE_LABELS = {
    "download": "Download",
    "quality": "Quality",
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


def _do_nothing() -> None:
    """Provide a safe callback for a disabled DearPyGui control."""


@dataclass(frozen=True, slots=True)
class WatchlistPage:
    items: tuple[WatchlistItem, ...]
    page: int
    page_count: int


def thumbnail_width_for_width(width: int) -> int:
    """Return a readable thumbnail width for the available list width."""
    if width < 750:
        return 180
    if width < 1050:
        return 240
    return 300


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
        on_thumbnail_needed: Callable[[str], None] | None = None,
        on_back: Callable[..., None],
        on_quit: Callable[..., None],
    ) -> None:
        self._on_add = on_add
        self._on_remove = on_remove
        self._on_refresh = on_refresh
        self._on_action = on_action
        self._on_thumbnail_needed = on_thumbnail_needed
        self._on_back = on_back
        self._on_quit = on_quit
        self._watchlist = Watchlist()
        self._request: WorkflowRequest | None = None
        self._selected_playlist_id: str | None = None
        self._filter = "All"
        self._page = 0
        self._thumbnail_width = 300
        self._textures: dict[str, Any] = {}
        self._thumbnail_requests: set[str] = set()

    def build(self, *, parent: int | str | None = None) -> None:
        """Build the watchlist content.

        With no *parent*, the content owns its own top-level window (used by
        standalone/test callers). With *parent* given, content is added
        directly into that container instead (used by the shell's tab bar) —
        DearPyGui containers always need a window ancestor somewhere, so an
        explicit `parent=` lets this nest inside one that already exists
        (the shell's primary window) without creating a second one.
        """
        if parent is None:
            with dpg.window(
                tag=WATCHLIST_ROOT,
                label="Playlist watchlist",
                on_close=self._on_back,
            ):
                self._build_content()
            dpg.set_primary_window(WATCHLIST_ROOT, True)
        else:
            with dpg.group(tag=WATCHLIST_ROOT, parent=parent):
                self._build_content()
        with dpg.texture_registry(tag=TEXTURE_REGISTRY):
            pass
        self._render()

    def _build_content(self) -> None:
        dpg.add_text("Add a YouTube playlist")
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

    def load(self, watchlist: Watchlist, request: WorkflowRequest) -> None:
        self._watchlist = watchlist
        self._request = request
        self._thumbnail_requests.clear()
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
        thumbnail_width = thumbnail_width_for_width(width)
        if thumbnail_width != self._thumbnail_width:
            self._thumbnail_width = thumbnail_width
            self._render_cards()

    def load_cached_thumbnail(self, video_id: str, path: Path) -> bool:
        """Create one texture. Call this method only on the render thread."""
        if video_id in self._textures or not path.is_file():
            self._thumbnail_requests.discard(video_id)
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
        self._thumbnail_requests.discard(video_id)
        self._render_cards()
        return True

    def release_textures(self) -> None:
        for tag in tuple(self._textures.values()):
            if dpg.does_item_exist(tag):
                dpg.delete_item(tag)
        self._textures.clear()
        self._thumbnail_requests.clear()

    def destroy(self) -> None:
        self.release_textures()
        if dpg.does_item_exist(TEXTURE_REGISTRY):
            dpg.delete_item(TEXTURE_REGISTRY)
        if dpg.does_item_exist(ACTIONS_WINDOW):
            dpg.delete_item(ACTIONS_WINDOW)
        if dpg.does_item_exist(WATCHLIST_ROOT):
            dpg.delete_item(WATCHLIST_ROOT)

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
        if not dpg.does_item_exist(WATCHLIST_ROOT):
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
                width=210,
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
            elif not playlist.last_error:
                dpg.add_text(
                    "Not checked yet",
                    color=NA_COLOR,
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
        visible_ids = {item.video_id for item in page.items if item.video_id}
        self._release_inactive_textures(visible_ids)
        self._thumbnail_requests.intersection_update(visible_ids)
        self._set_page_controls(page.page, page.page_count)
        if not page.items:
            if not playlist.items and playlist.last_checked_at is None:
                message = (
                    "This playlist has not been checked yet. Select Refresh new videos."
                )
            elif not playlist.items:
                message = "This playlist has no videos. Refresh it to check again."
            else:
                message = (
                    f"No videos have the {self._filter} status. "
                    "Select All to see every video."
                )
            dpg.set_value(EMPTY_TEXT, message)
            return
        dpg.set_value(EMPTY_TEXT, "")
        for item in page.items:
            self._add_row(playlist.playlist_id, item)

    def _add_row(self, playlist_id: str, item: WatchlistItem) -> None:
        thumbnail_height = round(self._thumbnail_width * 9 / 16)
        with dpg.child_window(
            tag=f"watchlist-row-{item.position}",
            parent=GRID,
            height=235,
            border=False,
            no_scrollbar=True,
        ):
            with dpg.table(
                header_row=False,
                policy=dpg.mvTable_SizingStretchProp,
                borders_innerV=False,
                no_pad_outerX=True,
            ):
                dpg.add_table_column(
                    width_fixed=True,
                    init_width_or_weight=self._thumbnail_width,
                )
                dpg.add_table_column(width_stretch=True)
                with dpg.table_row():
                    with dpg.table_cell():
                        self._add_thumbnail(item, thumbnail_height)
                    with dpg.table_cell():
                        self._add_item_details(playlist_id, item)

    def _add_thumbnail(self, item: WatchlistItem, height: int) -> None:
        texture = self._textures.get(item.video_id or "")
        if texture is not None:
            dpg.add_image(texture, width=self._thumbnail_width, height=height)
            return
        with dpg.child_window(
            width=self._thumbnail_width,
            height=height,
            border=False,
            no_scrollbar=True,
        ):
            dpg.add_spacer(height=max(24, height // 3))
            dpg.add_text("No cached thumbnail", color=STAGE_NOT_STARTED)
        if (
            item.video_id
            and self._on_thumbnail_needed is not None
            and item.video_id not in self._thumbnail_requests
        ):
            self._thumbnail_requests.add(item.video_id)
            self._on_thumbnail_needed(item.video_id)

    def _add_item_details(self, playlist_id: str, item: WatchlistItem) -> None:
        dpg.add_text(f"{item.position}. {item.title}", wrap=-1)
        summary = item_summary_state(item)
        summary_color = {
            "Processed": OK_COLOR,
            "Failed": FAIL_COLOR,
            "Processing": ACCENT,
        }.get(summary, STAGE_NOT_STARTED)
        with dpg.group(horizontal=True):
            dpg.add_text(
                f"YouTube ID: {item.video_id or 'Unavailable'}",
                color=STAGE_NOT_STARTED,
            )
            dpg.add_text(f"State: {summary}", color=summary_color)
        self._add_stage_rail(item)
        if item.last_error:
            dpg.add_text(item.last_error, color=FAIL_COLOR, wrap=-1)
        self._add_action_buttons(playlist_id, item)

    def _add_action_buttons(self, playlist_id: str, item: WatchlistItem) -> None:
        primary_action, primary_label = primary_item_action(item)
        actions = ((primary_action, primary_label),) + tuple(
            (action, ACTION_LABELS[action]) for action in _REPEAT_ACTIONS
        )
        with dpg.table(
            header_row=False,
            policy=dpg.mvTable_SizingStretchSame,
            borders_innerV=False,
            no_pad_outerX=True,
        ):
            for _ in range(3):
                dpg.add_table_column()
            for start in range(0, len(actions), 3):
                with dpg.table_row():
                    for index, (action, label) in enumerate(
                        actions[start : start + 3],
                        start=start,
                    ):
                        with dpg.table_cell():
                            if index == 0:
                                enabled = action is not None
                            else:
                                enabled = (
                                    action is not None
                                    and self._request is not None
                                    and item_action_availability(
                                        item,
                                        action,
                                        request=self._request,
                                    ).enabled
                                )
                            button = dpg.add_button(
                                label=label,
                                callback=(
                                    _do_nothing
                                    if action is None
                                    else partial(
                                        self._choose_action,
                                        playlist_id,
                                        item,
                                        action,
                                    )
                                ),
                                width=-1,
                                height=36,
                                enabled=enabled,
                            )
                            if index == 0 and action is not None:
                                bind_primary_button(button)

    def _add_stage_rail(self, item: WatchlistItem) -> None:
        with dpg.group(horizontal=True):
            for name in STAGE_NAMES:
                record = item.stages[name]
                dpg.add_text(
                    f"{_STAGE_LABELS[name]}: {_STATUS_LABELS[record.status]}",
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
                    callback=partial(
                        self._choose_action,
                        playlist_id,
                        item,
                        action,
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

    def _release_inactive_textures(self, visible_ids: set[str]) -> None:
        for video_id, tag in tuple(self._textures.items()):
            if video_id in visible_ids:
                continue
            if dpg.does_item_exist(tag):
                dpg.delete_item(tag)
            del self._textures[video_id]

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

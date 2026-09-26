"""Watchlist page: saved external playlist links and their item cards."""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass
from functools import partial
from math import ceil
from pathlib import Path
from typing import Any
import webbrowser

import dearpygui.dearpygui as dpg

from muzik.core.watchlist import (
    STAGE_NAMES,
    StageStatus,
    Watchlist,
    WatchlistItem,
    WatchlistPlaylist,
    WatchlistSourceKind,
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
    MUTED,
    NA_COLOR,
    OK_COLOR,
    STAGE_COMPLETE,
    STAGE_FAILED,
    STAGE_NOT_STARTED,
    STAGE_RUNNING,
    STAGE_SKIPPED,
    STAGE_STALE,
    bind_primary_button,
    build_card_theme,
)


WATCHLIST_ROOT = "watchlist-root"
PLAYLIST_RAIL = "watchlist-playlist-rail"
CONTENT_PANE = "watchlist-content"
GRID = "watchlist-grid"
ADD_URL = "watchlist-add-url"
ERROR_TEXT = "watchlist-error"
EMPTY_TEXT = "watchlist-empty"
FILTER = "watchlist-filter"
PAGE_TEXT = "watchlist-page-text"
COUNT_TEXT = "watchlist-count"
STATUS_TEXT = "watchlist-status"
PREVIOUS_BUTTON = "watchlist-previous"
NEXT_BUTTON = "watchlist-next"
REMOVE_BUTTON = "watchlist-remove"
REFRESH_BUTTON = "watchlist-refresh"
SPOTIFY_BUTTON = "watchlist-spotify"
RENAME_INPUT = "watchlist-rename"
TEXTURE_REGISTRY = "watchlist-textures"
ACTIONS_WINDOW = "watchlist-actions"

FILTERS = ("All", "Pending", "Processing", "Failed", "Processed", "Unavailable")
PAGE_SIZE = 8
RAIL_WIDTH = 240
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

_RAIL_WRAP = RAIL_WIDTH - 34
_RAIL_GUTTER = 34
_TEXT_LINE = 22
# One character of the interface font, in pixels. Card text is cut to one
# line, so a width in pixels has to become a length in characters.
_CHARACTER_WIDTH = 6.6
_STAGE_BAR_HEIGHT = 9
_STAGE_BAR_GAP = 3
_BAR_EMPTY = (58, 62, 70)
_KIND_LABELS = {
    WatchlistSourceKind.YOUTUBE: "YouTube",
    WatchlistSourceKind.SPOTIFY: "Spotify",
}
_KIND_COLORS = {
    WatchlistSourceKind.YOUTUBE: (230, 120, 120),
    WatchlistSourceKind.SPOTIFY: (120, 200, 120),
}

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
_SUMMARY_COLORS = {
    "Processed": OK_COLOR,
    "Failed": FAIL_COLOR,
    "Processing": ACCENT,
    "Unavailable": NA_COLOR,
}


def _do_nothing() -> None:
    """Provide a safe callback for a disabled DearPyGui control."""


@dataclass(frozen=True, slots=True)
class WatchlistPage:
    items: tuple[WatchlistItem, ...]
    page: int
    page_count: int


def thumbnail_width_for_width(width: int) -> int:
    """Return a readable thumbnail width for the available card width."""
    if width < 560:
        return 128
    if width < 900:
        return 160
    return 192


def card_area_width(viewport_width: int) -> int:
    """Return the width the cards can use, in 8 pixel steps.

    The rendered card area is used when it has one. It is quantized so that a
    one pixel change, such as a scrollbar that appears, cannot make the cards
    rebuild on every frame.
    """
    measured = 0
    if dpg.does_item_exist(GRID):
        measured = int(dpg.get_item_rect_size(GRID)[0])
    if measured < 200:
        measured = viewport_width - RAIL_WIDTH - _RAIL_GUTTER
    return max(320, measured // 8 * 8)


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


def shorten(text: str, limit: int) -> str:
    """Return *text* cut to *limit* characters, with a trailing ellipsis."""
    if len(text) <= limit:
        return text
    return f"{text[: max(1, limit - 3)].rstrip()}..."


def fit_characters(width: int) -> int:
    """Return how many characters of the interface font fit in *width*."""
    return max(16, int(width / _CHARACTER_WIDTH))


def short_timestamp(value: str) -> str:
    """Return the date and time of an ISO timestamp, without the offset."""
    date, separator, time = value.partition("T")
    if not separator:
        return date
    return f"{date} {time[:5]}"


class WatchlistView:
    """Own the render-thread widgets and textures for the watchlist page."""

    def __init__(
        self,
        *,
        on_add: Callable[[str], None],
        on_remove: Callable[[str], None],
        on_refresh: Callable[..., None],
        on_action: Callable[[str, WatchlistItem, ItemAction], None],
        on_rename: Callable[[str, str], None] | None = None,
        on_thumbnail_needed: Callable[[str, str], None] | None = None,
        on_spotify: Callable[..., None] | None = None,
        on_back: Callable[..., None] | None = None,
    ) -> None:
        self._on_add = on_add
        self._on_remove = on_remove
        self._on_refresh = on_refresh
        self._on_action = on_action
        self._on_rename = on_rename
        self._on_thumbnail_needed = on_thumbnail_needed
        self._on_spotify = on_spotify
        self._on_back = on_back
        self._watchlist = Watchlist()
        self._request: WorkflowRequest | None = None
        self._selected_playlist_id: str | None = None
        self._filter = "All"
        self._page = 0
        self._content_width = 640
        self._thumbnail_width = 192
        self._card_theme: str | int | None = None
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
        self._card_theme = build_card_theme()
        if parent is None:
            with dpg.window(
                tag=WATCHLIST_ROOT,
                label="Playlist watchlist",
                on_close=self._on_back or _do_nothing,
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
        with dpg.group(horizontal=True):
            dpg.add_input_text(
                tag=ADD_URL,
                hint="YouTube playlist URL or Spotify playlist link",
                width=-390,
                on_enter=True,
                callback=self._add,
            )
            dpg.add_button(label="Add source", callback=self._add, width=110)
            dpg.add_button(
                label="Spotify...",
                tag=SPOTIFY_BUTTON,
                callback=self._open_spotify,
                width=100,
                enabled=self._on_spotify is not None,
            )
            refresh = dpg.add_button(
                label="Refresh",
                tag=REFRESH_BUTTON,
                callback=self._on_refresh,
                width=160,
            )
            bind_primary_button(refresh)
        dpg.add_text("", tag=ERROR_TEXT, color=FAIL_COLOR, wrap=-1, show=False)
        with dpg.group(horizontal=True):
            with dpg.child_window(
                tag=PLAYLIST_RAIL,
                width=RAIL_WIDTH,
                height=-1,
                border=True,
            ):
                dpg.add_text("Sources", color=ACCENT)
            with dpg.child_window(
                tag=CONTENT_PANE,
                width=-1,
                height=-1,
                border=False,
            ):
                with dpg.group(horizontal=True):
                    dpg.add_combo(
                        FILTERS,
                        default_value="All",
                        tag=FILTER,
                        callback=self._filter_changed,
                        width=130,
                    )
                    dpg.add_button(
                        label="<",
                        tag=PREVIOUS_BUTTON,
                        callback=self._previous_page,
                        width=32,
                    )
                    dpg.add_text("Page 1 of 1", tag=PAGE_TEXT)
                    dpg.add_button(
                        label=">",
                        tag=NEXT_BUTTON,
                        callback=self._next_page,
                        width=32,
                    )
                    dpg.add_text("", tag=COUNT_TEXT, color=MUTED)
                    dpg.add_text("", tag=STATUS_TEXT, color=ACCENT, show=False)
                dpg.add_text("", tag=EMPTY_TEXT, wrap=-1, show=False)
                with dpg.child_window(tag=GRID, width=-1, height=-1, border=False):
                    pass

    def load(
        self,
        watchlist: Watchlist,
        request: WorkflowRequest,
        *,
        keep_page: bool = False,
    ) -> None:
        """Show *watchlist*. With *keep_page*, stay on the current page."""
        self._watchlist = watchlist
        self._request = request
        self._thumbnail_requests.clear()
        playlist_ids = {playlist.playlist_id for playlist in watchlist.playlists}
        if self._selected_playlist_id not in playlist_ids:
            self._selected_playlist_id = (
                watchlist.playlists[0].playlist_id if watchlist.playlists else None
            )
            keep_page = False
        if not keep_page:
            self._page = 0
        self._render()

    def show_error(self, message: str) -> None:
        dpg.set_value(ERROR_TEXT, message)
        dpg.configure_item(ERROR_TEXT, show=bool(message))

    def clear_error(self) -> None:
        dpg.set_value(ERROR_TEXT, "")
        dpg.configure_item(ERROR_TEXT, show=False)

    def set_status(self, message: str) -> None:
        """Show what a background job is doing, next to the paging controls."""
        if dpg.does_item_exist(STATUS_TEXT):
            dpg.set_value(STATUS_TEXT, message)
            dpg.configure_item(STATUS_TEXT, show=bool(message))

    def set_busy(self, busy: bool) -> None:
        if busy:
            dpg.disable_item(REFRESH_BUTTON)
        else:
            dpg.enable_item(REFRESH_BUTTON)

    def set_available_width(self, width: int) -> None:
        """Fit the cards to the card pane.

        The page sits inside the shell's tab bar, so the viewport width is
        only an estimate of the space the cards get. The rendered size of the
        card pane is the true value; *width* is the fallback for the frames
        before the pane has a size.
        """
        content_width = card_area_width(width)
        thumbnail_width = thumbnail_width_for_width(content_width)
        if (
            content_width == self._content_width
            and thumbnail_width == self._thumbnail_width
        ):
            return
        self._content_width = content_width
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
        if self._card_theme is not None and dpg.does_item_exist(self._card_theme):
            dpg.delete_item(self._card_theme)
        self._card_theme = None

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
        dpg.add_text("Sources", color=ACCENT, parent=PLAYLIST_RAIL)
        if not self._watchlist.playlists:
            dpg.add_text(
                "Add a YouTube playlist or a Spotify playlist link to start.",
                color=MUTED,
                wrap=_RAIL_WRAP,
                parent=PLAYLIST_RAIL,
            )
            return
        for playlist in self._watchlist.playlists:
            self._add_source_entry(playlist)
        self._add_source_footer()

    def _add_source_entry(self, playlist: WatchlistPlaylist) -> None:
        kind = playlist.source_kind
        dpg.add_selectable(
            label=shorten(playlist.display_name, 30),
            default_value=playlist.playlist_id == self._selected_playlist_id,
            callback=self._select_playlist,
            user_data=playlist.playlist_id,
            parent=PLAYLIST_RAIL,
            width=_RAIL_WRAP,
        )
        with dpg.group(horizontal=True, parent=PLAYLIST_RAIL):
            dpg.add_text(_KIND_LABELS[kind], color=_KIND_COLORS[kind])
            if kind is WatchlistSourceKind.YOUTUBE:
                dpg.add_text(f"{len(playlist.items)} video(s)", color=MUTED)
            else:
                dpg.add_text(f"{len(playlist.items)} track(s)", color=MUTED)
        if playlist.last_error:
            dpg.add_text(
                playlist.last_error,
                color=FAIL_COLOR,
                wrap=_RAIL_WRAP,
                parent=PLAYLIST_RAIL,
            )
        elif playlist.last_checked_at:
            dpg.add_text(
                f"Checked: {short_timestamp(playlist.last_checked_at)}",
                color=MUTED,
                wrap=_RAIL_WRAP,
                parent=PLAYLIST_RAIL,
            )
        else:
            dpg.add_text("Not checked yet", color=NA_COLOR, parent=PLAYLIST_RAIL)
        dpg.add_separator(parent=PLAYLIST_RAIL)

    def _add_source_footer(self) -> None:
        playlist = self._selected_playlist()
        if playlist is None:
            return
        dpg.add_text("Link", color=ACCENT, parent=PLAYLIST_RAIL)
        dpg.add_text(playlist.url, color=MUTED, wrap=_RAIL_WRAP, parent=PLAYLIST_RAIL)
        with dpg.group(horizontal=True, parent=PLAYLIST_RAIL):
            dpg.add_button(
                label="Open",
                width=(_RAIL_WRAP - 8) // 2,
                callback=partial(self._open_link, playlist.url),
            )
            dpg.add_button(
                label="Copy",
                width=(_RAIL_WRAP - 8) // 2,
                callback=partial(self._copy_link, playlist.url),
            )
        with dpg.group(horizontal=True, parent=PLAYLIST_RAIL):
            dpg.add_input_text(
                tag=RENAME_INPUT,
                default_value=playlist.title or "",
                hint="Name",
                width=_RAIL_WRAP - 74,
                on_enter=True,
                callback=self._rename,
            )
            dpg.add_button(label="Rename", width=66, callback=self._rename)
        dpg.add_button(
            label="Remove source",
            tag=REMOVE_BUTTON,
            callback=self._remove,
            width=_RAIL_WRAP,
            parent=PLAYLIST_RAIL,
        )

    def _render_cards(self) -> None:
        if not dpg.does_item_exist(GRID):
            return
        dpg.delete_item(GRID, children_only=True)
        playlist = self._selected_playlist()
        if playlist is None:
            self._set_empty_message("No source is selected.")
            dpg.set_value(COUNT_TEXT, "")
            self._set_page_controls(0, 1)
            return
        if playlist.source_kind is WatchlistSourceKind.SPOTIFY and not playlist.items:
            self._render_spotify_start(playlist)
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
        unit = (
            "track" if playlist.source_kind is WatchlistSourceKind.SPOTIFY else "video"
        )
        dpg.set_value(COUNT_TEXT, f"{len(playlist.items)} {unit}(s)")
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
            self._set_empty_message(message)
            return
        self._set_empty_message("")
        for item in page.items:
            self._add_row(playlist.playlist_id, item)

    def _render_spotify_start(self, playlist: WatchlistPlaylist) -> None:
        """Explain what a Spotify source needs before its first sync."""
        self._set_page_controls(0, 1)
        dpg.set_value(COUNT_TEXT, "")
        self._set_empty_message("")
        wrap = self._content_width - 40
        with dpg.child_window(parent=GRID, height=190, border=False) as card:
            dpg.add_text(playlist.display_name, color=ACCENT, wrap=wrap)
            if playlist.last_error:
                dpg.add_text(playlist.last_error, color=FAIL_COLOR, wrap=wrap)
            dpg.add_text(
                "This Spotify source has no tracks yet. Select Refresh to read "
                "it with the Spotify Web API.",
                wrap=wrap,
            )
            dpg.add_text(
                "muzik reads the track names only. The audio comes from "
                "Soulseek, thus set the audio source to Soulseek in the "
                "Workflow tab.",
                color=MUTED,
                wrap=wrap,
            )
            with dpg.group(horizontal=True):
                dpg.add_button(
                    label="Spotify...",
                    width=110,
                    enabled=self._on_spotify is not None,
                    callback=self._open_spotify,
                )
                dpg.add_button(
                    label="Open in Spotify",
                    width=160,
                    callback=partial(self._open_link, playlist.url),
                )
        if self._card_theme is not None:
            dpg.bind_item_theme(card, self._card_theme)

    def _set_empty_message(self, message: str) -> None:
        dpg.set_value(EMPTY_TEXT, message)
        dpg.configure_item(EMPTY_TEXT, show=bool(message))

    def _card_height(self, item: WatchlistItem) -> int:
        text_lines = 2 + (1 if item.last_error else 0)
        body = text_lines * _TEXT_LINE + _STAGE_BAR_HEIGHT + 14 + 32
        thumbnail = round(self._thumbnail_width * 9 / 16) + 6
        return max(thumbnail, body) + 8

    def _details_width(self) -> int:
        return max(200, self._content_width - self._thumbnail_width - 56)

    def _add_row(self, playlist_id: str, item: WatchlistItem) -> None:
        thumbnail_height = round(self._thumbnail_width * 9 / 16)
        with dpg.child_window(
            tag=f"watchlist-row-{item.position}",
            parent=GRID,
            height=self._card_height(item),
            border=False,
            no_scrollbar=True,
        ) as card:
            with dpg.group(horizontal=True):
                self._add_thumbnail(item, thumbnail_height)
                with dpg.group():
                    self._add_item_details(playlist_id, item)
        if self._card_theme is not None:
            dpg.bind_item_theme(card, self._card_theme)

    def _add_thumbnail(self, item: WatchlistItem, height: int) -> None:
        texture = self._textures.get(item.video_id or "")
        if texture is not None:
            dpg.add_image(texture, width=self._thumbnail_width, height=height)
            return
        with dpg.child_window(
            width=self._thumbnail_width,
            height=height,
            border=True,
            no_scrollbar=True,
        ):
            dpg.add_spacer(height=max(2, height // 2 - 26))
            dpg.add_text(
                "No cached thumbnail",
                color=MUTED,
                wrap=self._thumbnail_width - 26,
            )
        if (
            item.video_id
            and item.thumbnail_url
            and self._on_thumbnail_needed is not None
            and item.video_id not in self._thumbnail_requests
        ):
            self._thumbnail_requests.add(item.video_id)
            self._on_thumbnail_needed(item.video_id, item.thumbnail_url)

    def _add_item_details(self, playlist_id: str, item: WatchlistItem) -> None:
        details_width = self._details_width()
        summary = item_summary_state(item)
        dpg.add_text(
            shorten(f"{item.position}. {item.title}", fit_characters(details_width))
        )
        with dpg.group(horizontal=True):
            state_text = f"State: {summary}"
            dpg.add_text(state_text, color=_SUMMARY_COLORS.get(summary, MUTED))
            label = (
                "Spotify ID"
                if item.source_kind is WatchlistSourceKind.SPOTIFY
                else "YouTube ID"
            )
            dpg.add_text(
                shorten(
                    f"{label}: {item.video_id or 'Unavailable'}",
                    fit_characters(details_width) - len(state_text) - 1,
                ),
                color=MUTED,
            )
        self._add_stage_rail(item, details_width)
        if item.last_error:
            dpg.add_text(
                shorten(item.last_error, fit_characters(details_width)),
                color=FAIL_COLOR,
            )
        self._add_action_buttons(playlist_id, item)

    def _add_action_buttons(self, playlist_id: str, item: WatchlistItem) -> None:
        primary_action, primary_label = primary_item_action(item)
        with dpg.group(horizontal=True):
            button = dpg.add_button(
                label=primary_label,
                width=120,
                height=30,
                enabled=primary_action is not None,
                callback=(
                    _do_nothing
                    if primary_action is None
                    else partial(
                        self._choose_action,
                        playlist_id,
                        item,
                        primary_action,
                    )
                ),
            )
            if primary_action is not None:
                bind_primary_button(button)
            dpg.add_button(
                label="More actions",
                width=130,
                height=30,
                callback=partial(self._open_actions, playlist_id, item),
            )

    def _add_stage_rail(self, item: WatchlistItem, width: int) -> None:
        segment = max(14, (width - 40) // len(STAGE_NAMES))
        bar_width = segment * len(STAGE_NAMES) + _STAGE_BAR_GAP * (len(STAGE_NAMES) - 1)
        with dpg.group() as bar:
            with dpg.drawlist(width=bar_width, height=_STAGE_BAR_HEIGHT):
                for index, name in enumerate(STAGE_NAMES):
                    status = item.stages[name].status
                    color = (
                        _BAR_EMPTY
                        if status is StageStatus.NOT_STARTED
                        else _STATUS_COLORS[status]
                    )
                    left = index * (segment + _STAGE_BAR_GAP)
                    dpg.draw_rectangle(
                        (left, 0),
                        (left + segment, _STAGE_BAR_HEIGHT),
                        color=color,
                        fill=color,
                        rounding=2,
                    )
        with dpg.tooltip(bar):
            for name in STAGE_NAMES:
                status = item.stages[name].status
                dpg.add_text(
                    f"{_STAGE_LABELS[name]}: {_STATUS_LABELS[status]}",
                    color=_STATUS_COLORS[status],
                )

    def _open_actions(
        self,
        playlist_id: str,
        item: WatchlistItem,
        sender: Any = None,
        app_data: Any = None,
        user_data: Any = None,
    ) -> None:
        if dpg.does_item_exist(ACTIONS_WINDOW):
            dpg.delete_item(ACTIONS_WINDOW)
        with dpg.window(
            tag=ACTIONS_WINDOW,
            label="Commands",
            modal=True,
            width=560,
            height=520,
            on_close=self._close_actions,
        ):
            is_spotify = item.source_kind is WatchlistSourceKind.SPOTIFY
            dpg.add_text(item.title, wrap=520)
            dpg.add_text(
                f"{'Spotify' if is_spotify else 'YouTube'} ID: "
                f"{item.video_id or 'Unavailable'}",
                color=MUTED,
            )
            if item.video_url:
                dpg.add_button(
                    label="Open in Spotify" if is_spotify else "Open on YouTube",
                    width=180,
                    callback=partial(self._open_link, item.video_url),
                )
            dpg.add_separator()
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
                    height=32,
                    enabled=available.enabled,
                    callback=partial(
                        self._choose_action,
                        playlist_id,
                        item,
                        action,
                    ),
                )
                if not available.enabled and available.reason:
                    dpg.add_text(available.reason, color=MUTED, wrap=500)
            dpg.add_separator()
            dpg.add_button(label="Close", callback=self._close_actions, width=100)

    def _choose_action(
        self,
        playlist_id: str,
        item: WatchlistItem,
        action: ItemAction,
        sender: Any = None,
        app_data: Any = None,
        user_data: Any = None,
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

    def _open_spotify(
        self,
        sender: Any = None,
        app_data: Any = None,
        user_data: Any = None,
    ) -> None:
        if self._on_spotify is not None:
            self._on_spotify()

    def _open_link(
        self,
        url: str,
        sender: Any = None,
        app_data: Any = None,
        user_data: Any = None,
    ) -> None:
        webbrowser.open(url)

    def _copy_link(
        self,
        url: str,
        sender: Any = None,
        app_data: Any = None,
        user_data: Any = None,
    ) -> None:
        dpg.set_clipboard_text(url)

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
            self.show_error("Enter a YouTube playlist URL or a Spotify playlist link.")
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

    def _rename(
        self,
        sender: Any = None,
        app_data: Any = None,
        user_data: Any = None,
    ) -> None:
        if self._selected_playlist_id is None or self._on_rename is None:
            return
        self._on_rename(self._selected_playlist_id, str(dpg.get_value(RENAME_INPUT)))

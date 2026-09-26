"""Spotify connection window: client ID, login, and playlist selection."""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass, field
from typing import Any

import dearpygui.dearpygui as dpg

from muzik.core.sources.spotify_api import LIKED_URI, SpotifyPlaylistRef
from muzik.gui.theme import ACCENT, FAIL_COLOR, MUTED, OK_COLOR, bind_primary_button


SPOTIFY_WINDOW = "spotify-window"
SPOTIFY_BODY = "spotify-body"
CLIENT_ID_INPUT = "spotify-client-id"

_DASHBOARD_URL = "https://developer.spotify.com/dashboard"
_WINDOW_WIDTH = 620
_WRAP = 560


def _centered_position() -> list[int]:
    """Return a position near the top centre of the viewport.

    A test builds this window with no viewport, thus a missing viewport gives
    the default position instead of an error.
    """
    try:
        width = dpg.get_viewport_client_width()
    except Exception:
        return []
    return [max(20, (width - _WINDOW_WIDTH) // 2), 70]


@dataclass(frozen=True, slots=True)
class SpotifyState:
    """What the Spotify window shows right now."""

    client_id: str = ""
    redirect_uri: str = ""
    connected: bool = False
    account: str = ""
    playlists: tuple[SpotifyPlaylistRef, ...] = ()
    saved_uris: frozenset[str] = field(default_factory=frozenset)
    status: str = ""
    error: str = ""
    busy: bool = False


class SpotifyDialog:
    """Own the render-thread widgets of the Spotify connection window."""

    def __init__(
        self,
        *,
        on_save_client_id: Callable[[str], None],
        on_connect: Callable[[], None],
        on_disconnect: Callable[[], None],
        on_reload: Callable[[], None],
        on_add: Callable[[str], None],
        on_copy: Callable[[str], None],
        on_open_link: Callable[[str], object] | None = None,
    ) -> None:
        self._on_save_client_id = on_save_client_id
        self._on_connect = on_connect
        self._on_disconnect = on_disconnect
        self._on_reload = on_reload
        self._on_add = on_add
        self._on_copy = on_copy
        self._on_open_link = on_open_link or (lambda url: None)
        self._state = SpotifyState()

    @property
    def is_open(self) -> bool:
        return dpg.does_item_exist(SPOTIFY_WINDOW)

    def open(self, state: SpotifyState) -> None:
        """Build the window and show *state*.

        The window fits its own content, because the setup step is short and
        the playlist step is tall. A fixed height leaves one of the two with
        a large empty area.
        """
        self.close()
        with dpg.window(
            tag=SPOTIFY_WINDOW,
            label="Spotify",
            modal=True,
            autosize=True,
            pos=_centered_position(),
            on_close=self.close,
        ):
            with dpg.group(tag=SPOTIFY_BODY):
                pass
        self.show(state)

    def close(
        self,
        sender: Any = None,
        app_data: Any = None,
        user_data: Any = None,
    ) -> None:
        if dpg.does_item_exist(SPOTIFY_WINDOW):
            dpg.delete_item(SPOTIFY_WINDOW)

    def show(self, state: SpotifyState) -> None:
        """Replace the content of the window with *state*."""
        self._state = state
        if not dpg.does_item_exist(SPOTIFY_BODY):
            return
        dpg.delete_item(SPOTIFY_BODY, children_only=True)
        self._build_setup(state)
        if state.connected:
            self._build_account(state)
            self._build_playlists(state)
        elif state.client_id:
            connect = dpg.add_button(
                label="Connect to Spotify",
                width=200,
                height=32,
                enabled=not state.busy,
                callback=self._connect,
                parent=SPOTIFY_BODY,
            )
            bind_primary_button(connect)
            dpg.add_text(
                "muzik opens your browser. Approve the request, then come back.",
                color=MUTED,
                wrap=_WRAP,
                parent=SPOTIFY_BODY,
            )
        if state.status:
            dpg.add_text(state.status, color=MUTED, wrap=_WRAP, parent=SPOTIFY_BODY)
        if state.error:
            dpg.add_text(state.error, color=FAIL_COLOR, wrap=_WRAP, parent=SPOTIFY_BODY)
        dpg.add_separator(parent=SPOTIFY_BODY)
        dpg.add_button(
            label="Close", width=100, callback=self.close, parent=SPOTIFY_BODY
        )

    def _build_setup(self, state: SpotifyState) -> None:
        dpg.add_text("Your Spotify application", color=ACCENT, parent=SPOTIFY_BODY)
        dpg.add_text(
            "muzik has no Spotify application of its own. Create one, give it "
            "the redirect URI below, then paste its client ID here.",
            color=MUTED,
            wrap=_WRAP,
            parent=SPOTIFY_BODY,
        )
        with dpg.group(horizontal=True, parent=SPOTIFY_BODY):
            dpg.add_button(
                label="Open the Spotify dashboard",
                width=240,
                callback=lambda *_: self._on_open_link(_DASHBOARD_URL),
            )
            dpg.add_button(
                label="Copy link",
                width=110,
                callback=lambda *_: self._on_copy(_DASHBOARD_URL),
            )
        with dpg.group(horizontal=True, parent=SPOTIFY_BODY):
            dpg.add_input_text(
                tag=CLIENT_ID_INPUT,
                default_value=state.client_id,
                hint="Client ID",
                width=400,
                on_enter=True,
                callback=self._save_client_id,
            )
            dpg.add_button(
                label="Save client ID", width=150, callback=self._save_client_id
            )
        with dpg.group(horizontal=True, parent=SPOTIFY_BODY):
            dpg.add_input_text(
                default_value=state.redirect_uri,
                width=400,
                readonly=True,
            )
            dpg.add_button(
                label="Copy redirect URI",
                width=150,
                callback=lambda *_: self._on_copy(state.redirect_uri),
            )
        dpg.add_text(
            "Add that redirect URI to the application, character for "
            "character. 'localhost' does not match '127.0.0.1'.",
            color=MUTED,
            wrap=_WRAP,
            parent=SPOTIFY_BODY,
        )
        dpg.add_separator(parent=SPOTIFY_BODY)

    def _build_account(self, state: SpotifyState) -> None:
        with dpg.group(horizontal=True, parent=SPOTIFY_BODY):
            dpg.add_text(
                f"Connected as {state.account or 'your account'}", color=OK_COLOR
            )
            dpg.add_button(
                label="Reload",
                width=90,
                enabled=not state.busy,
                callback=lambda *_: self._on_reload(),
            )
            dpg.add_button(
                label="Disconnect",
                width=110,
                callback=lambda *_: self._on_disconnect(),
            )

    def _build_playlists(self, state: SpotifyState) -> None:
        dpg.add_text("Add a source", color=ACCENT, parent=SPOTIFY_BODY)
        if state.busy and not state.playlists:
            dpg.add_text("Reading your playlists...", color=MUTED, parent=SPOTIFY_BODY)
            return
        if not state.playlists:
            dpg.add_text(
                "No playlists were returned for this account.",
                color=MUTED,
                wrap=_WRAP,
                parent=SPOTIFY_BODY,
            )
            return
        with dpg.child_window(width=_WRAP + 20, height=240, parent=SPOTIFY_BODY):
            with dpg.table(
                header_row=True,
                policy=dpg.mvTable_SizingStretchProp,
                borders_innerH=True,
            ):
                dpg.add_table_column(label="Playlist")
                dpg.add_table_column(
                    label="Tracks", width_fixed=True, init_width_or_weight=70
                )
                dpg.add_table_column(
                    label="", width_fixed=True, init_width_or_weight=90
                )
                for reference in state.playlists:
                    with dpg.table_row():
                        label = reference.name
                        if reference.uri == LIKED_URI:
                            label = f"{reference.name} (your library)"
                        dpg.add_text(label)
                        dpg.add_text(
                            "" if reference.total is None else str(reference.total),
                            color=MUTED,
                        )
                        saved = reference.uri in state.saved_uris
                        dpg.add_button(
                            label="Added" if saved else "Add",
                            width=-1,
                            enabled=not saved,
                            callback=self._add_callback(reference.uri),
                        )

    def _add_callback(self, uri: str) -> Callable[..., None]:
        def callback(
            sender: Any = None,
            app_data: Any = None,
            user_data: Any = None,
        ) -> None:
            self._on_add(uri)

        return callback

    def _connect(
        self,
        sender: Any = None,
        app_data: Any = None,
        user_data: Any = None,
    ) -> None:
        self._on_connect()

    def _save_client_id(
        self,
        sender: Any = None,
        app_data: Any = None,
        user_data: Any = None,
    ) -> None:
        if dpg.does_item_exist(CLIENT_ID_INPUT):
            self._on_save_client_id(str(dpg.get_value(CLIENT_ID_INPUT)).strip())

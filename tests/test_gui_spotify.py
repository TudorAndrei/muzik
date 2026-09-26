from typing import Any, cast

import dearpygui.dearpygui as dpg

from muzik.core.sources.spotify_api import LIKED_URI, SpotifyPlaylistRef
from muzik.gui.spotify import SPOTIFY_BODY, CLIENT_ID_INPUT, SpotifyDialog, SpotifyState


def _dialog(**callbacks: Any) -> SpotifyDialog:
    defaults: dict[str, Any] = {
        "on_save_client_id": lambda value: None,
        "on_connect": lambda: None,
        "on_disconnect": lambda: None,
        "on_reload": lambda: None,
        "on_add": lambda uri: None,
        "on_copy": lambda text: None,
        "on_open_link": lambda url: None,
    }
    defaults.update(callbacks)
    return SpotifyDialog(**defaults)


def _descendants(parent) -> list[int | str]:
    result: list[int | str] = []
    pending = [parent]
    while pending:
        current = pending.pop()
        children = cast(dict[int, list[int | str]], dpg.get_item_children(current))
        for slot in children.values():
            result.extend(slot)
            pending.extend(slot)
    return result


def _text_values(parent) -> list[str]:
    values: list[str] = []
    for item in _descendants(parent):
        value = dpg.get_value(item)
        if isinstance(value, str):
            values.append(value)
    return values


def _labels(parent) -> set[str | None]:
    return {dpg.get_item_label(item) for item in _descendants(parent)}


def test_without_a_client_id_the_window_asks_for_one() -> None:
    dpg.create_context()
    opened: list[str] = []
    dialog = _dialog(on_open_link=opened.append)
    try:
        dialog.open(SpotifyState(redirect_uri="http://127.0.0.1:8888/callback"))

        text = _text_values(SPOTIFY_BODY)
        labels = _labels(SPOTIFY_BODY)
        assert "http://127.0.0.1:8888/callback" in text
        assert any("does not match" in value for value in text)
        assert {"Save client ID", "Copy redirect URI"} <= labels
        assert "Connect to Spotify" not in labels

        dashboard = next(
            item
            for item in _descendants(SPOTIFY_BODY)
            if dpg.get_item_label(item) == "Open the Spotify dashboard"
        )
        callback = dpg.get_item_callback(dashboard)
        assert callback is not None
        dpg.run_callbacks([[callback, dashboard, None, None]])

        assert opened == ["https://developer.spotify.com/dashboard"]
    finally:
        dialog.close()
        dpg.destroy_context()


def test_a_saved_client_id_offers_the_login() -> None:
    dpg.create_context()
    connected: list[bool] = []
    dialog = _dialog(on_connect=lambda: connected.append(True))
    try:
        dialog.open(SpotifyState(client_id="client-1"))
        button = next(
            item
            for item in _descendants(SPOTIFY_BODY)
            if dpg.get_item_label(item) == "Connect to Spotify"
        )
        callback = dpg.get_item_callback(button)
        assert callback is not None
        dpg.run_callbacks([[callback, button, None, None]])

        assert connected == [True]
    finally:
        dialog.close()
        dpg.destroy_context()


def test_a_connected_account_lists_playlists_and_marks_saved_sources() -> None:
    dpg.create_context()
    added: list[str] = []
    dialog = _dialog(on_add=added.append)
    try:
        dialog.open(
            SpotifyState(
                client_id="client-1",
                connected=True,
                account="Tudor",
                playlists=(
                    SpotifyPlaylistRef(uri=LIKED_URI, name="Liked Songs"),
                    SpotifyPlaylistRef(
                        uri="spotify:playlist:PL1", name="Road trip", total=12
                    ),
                ),
                saved_uris=frozenset({LIKED_URI}),
            )
        )
        text = _text_values(SPOTIFY_BODY)

        assert "Connected as Tudor" in text
        assert "Liked Songs (your library)" in text
        assert "Road trip" in text
        assert {"Add", "Added", "Disconnect", "Reload"} <= _labels(SPOTIFY_BODY)

        add_button = next(
            item
            for item in _descendants(SPOTIFY_BODY)
            if dpg.get_item_label(item) == "Add"
        )
        callback = dpg.get_item_callback(add_button)
        assert callback is not None
        dpg.run_callbacks([[callback, add_button, None, None]])

        assert added == ["spotify:playlist:PL1"]
    finally:
        dialog.close()
        dpg.destroy_context()


def test_saving_a_client_id_sends_the_trimmed_value() -> None:
    dpg.create_context()
    saved: list[str] = []
    dialog = _dialog(on_save_client_id=saved.append)
    try:
        dialog.open(SpotifyState())
        dpg.set_value(CLIENT_ID_INPUT, "  client-1  ")
        dialog._save_client_id()

        assert saved == ["client-1"]
    finally:
        dialog.close()
        dpg.destroy_context()


def test_an_error_is_shown_and_the_window_can_close() -> None:
    dpg.create_context()
    dialog = _dialog()
    try:
        dialog.open(SpotifyState(client_id="client-1", error="Spotify: Forbidden"))
        assert "Spotify: Forbidden" in _text_values(SPOTIFY_BODY)

        dialog.close()

        assert dialog.is_open is False
    finally:
        dpg.destroy_context()

from pathlib import Path
from typing import Any, cast

import dearpygui.dearpygui as dpg

from muzik.core.watchlist import (
    StageRecord,
    StageStatus,
    Watchlist,
    WatchlistItem,
    WatchlistPlaylist,
)
from muzik.core.workflow.item_actions import ItemAction
from muzik.core.workflow.service import WorkflowRequest
from muzik.gui.watchlist import (
    ACTIONS_WINDOW,
    ADD_URL,
    FILTER,
    GRID,
    PAGE_TEXT,
    PLAYLIST_RAIL,
    TEXTURE_REGISTRY,
    WatchlistView,
    columns_for_width,
    page_for_items,
)


def _item(
    position: int,
    *,
    status: StageStatus = StageStatus.NOT_STARTED,
    available: bool = True,
) -> WatchlistItem:
    video_id = f"video{position:06d}" if available else None
    item = WatchlistItem(
        position=position,
        title=f"Video {position}",
        video_id=video_id,
        video_url=(f"https://youtu.be/{video_id}" if video_id else None),
    )
    item.stages["download"] = StageRecord(status=status)
    if status is StageStatus.COMPLETE:
        for record in item.stages.values():
            record.status = StageStatus.COMPLETE
    return item


def _view(**callbacks: Any) -> WatchlistView:
    defaults: dict[str, Any] = {
        "on_add": lambda value: None,
        "on_remove": lambda value: None,
        "on_refresh": lambda: None,
        "on_action": lambda playlist_id, item, action: None,
        "on_back": lambda: None,
        "on_quit": lambda: None,
    }
    defaults.update(callbacks)
    return WatchlistView(**defaults)


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


def test_columns_follow_available_width() -> None:
    assert columns_for_width(200) == 1
    assert columns_for_width(700) == 2
    assert columns_for_width(1000) == 3
    assert columns_for_width(1800) == 4


def test_page_filters_summary_state_and_clamps_page() -> None:
    items = [_item(index) for index in range(1, 8)]
    items[1] = _item(2, status=StageStatus.FAILED)
    items[2] = _item(3, status=StageStatus.COMPLETE)
    items[3] = _item(4, available=False)

    failed = page_for_items(items, status_filter="Failed", page=0)
    last_page = page_for_items(items, status_filter="All", page=20)

    assert [item.position for item in failed.items] == [2]
    assert last_page.page == 1
    assert [item.position for item in last_page.items] == [7]


def test_unchecked_playlist_explains_how_to_load_videos(tmp_path: Path) -> None:
    playlist = WatchlistPlaylist(
        "PL_NOT_CHECKED",
        "https://www.youtube.com/playlist?list=PL_NOT_CHECKED",
    )

    dpg.create_context()
    view = _view()
    try:
        view.build()
        view.load(
            Watchlist([playlist]),
            WorkflowRequest("", tmp_path / "downloads", tmp_path / "splits"),
        )

        assert dpg.get_value("watchlist-empty") == (
            "This playlist has not been checked yet. Select Refresh new videos."
        )
        assert "Not checked yet" in _text_values(PLAYLIST_RAIL)
    finally:
        view.destroy()
        dpg.destroy_context()


def test_cards_show_item_data_stage_rail_and_placeholder(tmp_path: Path) -> None:
    item = _item(1, status=StageStatus.FAILED)
    item.stages["parse"] = StageRecord(status=StageStatus.STALE)
    item.last_error = "The download failed."
    playlist = WatchlistPlaylist("PL123", "https://example.test", items=[item])

    dpg.create_context()
    view = _view()
    try:
        view.build()
        view.load(
            Watchlist([playlist]),
            WorkflowRequest("", tmp_path / "downloads", tmp_path / "splits"),
        )
        text = _text_values(GRID)

        assert "1. Video 1" in text
        assert "YouTube ID: video000001" in text
        assert "State: Failed" in text
        assert "Download" in text
        assert "Failed" in text
        assert "Parse" in text
        assert "Stale" in text
        assert "Split" in text
        assert "Organize" in text
        assert "No cached thumbnail" in text
        assert "The download failed." in text
    finally:
        view.destroy()
        dpg.destroy_context()


def test_paging_and_status_filter_replace_visible_cards(tmp_path: Path) -> None:
    items = [_item(index) for index in range(1, 8)]
    items[0].stages["download"].status = StageStatus.FAILED
    playlist = WatchlistPlaylist("PL123", "https://example.test", items=items)

    dpg.create_context()
    view = _view()
    try:
        view.build()
        view.load(
            Watchlist([playlist]),
            WorkflowRequest("", tmp_path / "downloads", tmp_path / "splits"),
        )
        assert "1. Video 1" in _text_values(GRID)
        assert "7. Video 7" not in _text_values(GRID)

        view._next_page()
        assert dpg.get_value(PAGE_TEXT) == "Page 2 of 2"
        assert "7. Video 7" in _text_values(GRID)

        dpg.set_value(FILTER, "Failed")
        view._filter_changed(app_data="Failed")
        assert dpg.get_value(PAGE_TEXT) == "Page 1 of 1"
        assert "1. Video 1" in _text_values(GRID)
        assert "2. Video 2" not in _text_values(GRID)
    finally:
        view.destroy()
        dpg.destroy_context()


def test_add_remove_and_item_action_callbacks(tmp_path: Path) -> None:
    added: list[str] = []
    removed: list[str] = []
    actions: list[tuple[str, int, ItemAction]] = []
    item = _item(1)
    playlist = WatchlistPlaylist("PL123", "https://example.test", items=[item])

    dpg.create_context()
    view = _view(
        on_add=added.append,
        on_remove=removed.append,
        on_action=lambda playlist_id, selected, action: actions.append(
            (playlist_id, selected.position, action)
        ),
    )
    try:
        view.build()
        view.load(
            Watchlist([playlist]),
            WorkflowRequest("", tmp_path / "downloads", tmp_path / "splits"),
        )
        dpg.set_value(ADD_URL, " https://youtube.test/playlist ")
        view._add()
        view._remove()
        view._choose_action("PL123", item, ItemAction.RUN)

        assert added == ["https://youtube.test/playlist"]
        assert removed == ["PL123"]
        assert actions == [("PL123", 1, ItemAction.RUN)]
    finally:
        view.destroy()
        dpg.destroy_context()


def test_actions_show_disabled_input_reason(tmp_path: Path) -> None:
    item = _item(1)
    playlist = WatchlistPlaylist("PL123", "https://example.test", items=[item])

    dpg.create_context()
    view = _view()
    try:
        view.build()
        view.load(
            Watchlist([playlist]),
            WorkflowRequest("", tmp_path / "downloads", tmp_path / "splits"),
        )
        view._open_actions("PL123", item)

        text = _text_values(ACTIONS_WINDOW)
        assert "Download this video before you run this command." in text
        assert "Parse and accept chapters before you split this video." not in text
    finally:
        view.destroy()
        dpg.destroy_context()


def test_texture_is_released_when_view_is_destroyed(
    tmp_path: Path,
    monkeypatch,
) -> None:
    image = tmp_path / "thumbnail.png"
    image.write_bytes(b"image bytes")
    monkeypatch.setattr(dpg, "load_image", lambda path: (1, 1, 4, [1.0] * 4))

    dpg.create_context()
    view = _view()
    try:
        view.build()
        assert view.load_cached_thumbnail("video000001", image) is True
        texture = view._textures["video000001"]
        assert dpg.does_item_exist(texture)

        view.destroy()

        assert not dpg.does_item_exist(texture)
        assert not dpg.does_item_exist(TEXTURE_REGISTRY)
    finally:
        dpg.destroy_context()


def test_page_requests_only_visible_textures_and_releases_old_page(
    tmp_path: Path,
    monkeypatch,
) -> None:
    requests: list[str] = []
    playlist = WatchlistPlaylist(
        "PL123",
        "https://example.test",
        items=[_item(index) for index in range(1, 8)],
    )
    image = tmp_path / "thumbnail.png"
    image.write_bytes(b"image bytes")
    monkeypatch.setattr(dpg, "load_image", lambda path: (1, 1, 4, [1.0] * 4))

    dpg.create_context()
    view = _view(on_thumbnail_needed=requests.append)
    try:
        view.build()
        view.load(
            Watchlist([playlist]),
            WorkflowRequest("", tmp_path / "downloads", tmp_path / "splits"),
        )
        assert requests == [f"video{index:06d}" for index in range(1, 7)]
        view.load_cached_thumbnail("video000001", image)
        old_texture = view._textures["video000001"]

        view._next_page()

        assert requests[-1] == "video000007"
        assert "video000001" not in view._textures
        assert not dpg.does_item_exist(old_texture)
    finally:
        view.destroy()
        dpg.destroy_context()

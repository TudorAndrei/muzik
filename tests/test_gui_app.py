from threading import Event
import time
from pathlib import Path

import dearpygui.dearpygui as dpg

from muzik.core.watchlist import (
    Watchlist,
    WatchlistItem,
    WatchlistPlaylist,
    WatchlistRepository,
)
from muzik.core.workflow.cancellation import WorkflowCancelled
from muzik.core.workflow.item_actions import ItemAction, ItemActionOperations
from muzik.core.workflow.launch import WorkflowLaunchConfig
from muzik.core.workflow.service import WorkflowRunOperations
from muzik.gui.launcher import FIELD_TAGS
from muzik.gui.app import MuzikGuiApp
from muzik.gui.watchlist import WATCHLIST_WINDOW


def test_back_waits_for_worker_then_returns_to_launcher() -> None:
    started = Event()
    stopped = Event()

    def operations_factory(config, decisions, events):
        def process_audio(audio_inputs, pre_split_dirs, *, cancellation=None):
            started.set()
            while cancellation is not None and not cancellation.is_cancelled():
                time.sleep(0.005)
            stopped.set()
            raise WorkflowCancelled("Workflow cancelled.")

        return WorkflowRunOperations(
            download_audio=lambda *args: True,
            process_audio=process_audio,
            acquire_soulseek=lambda raw: [],
            prepopulate_archive=lambda archive: None,
            get_playlist_video_ids=lambda raw: [],
        )

    dpg.create_context()
    app = MuzikGuiApp(operations_factory=operations_factory)
    try:
        app.launcher.build()
        app.open_pipeline(WorkflowLaunchConfig(raw="local-input", dry_run=True))
        assert started.wait(1)

        app.back()

        assert app.pipeline is not None
        assert stopped.wait(1)
        app._poll_worker()
        assert app.pipeline is None
    finally:
        app.bridge.shutdown()
        dpg.destroy_context()


def _repository(tmp_path: Path, *, with_item: bool = False) -> WatchlistRepository:
    repository = WatchlistRepository(tmp_path / "watchlist.json")
    item = WatchlistItem(
        position=1,
        title="Test video",
        video_id="video000001",
        video_url="https://youtu.be/video000001",
        thumbnail_url="https://example.test/thumb.jpg",
    )
    repository.save(
        Watchlist(
            [
                WatchlistPlaylist(
                    "PL123",
                    "https://www.youtube.com/playlist?list=PL123",
                    items=[item] if with_item else [],
                )
            ]
        )
    )
    return repository


def test_watchlist_navigation_uses_saved_data_and_releases_view(tmp_path: Path) -> None:
    repository = _repository(tmp_path, with_item=True)
    dpg.create_context()
    app = MuzikGuiApp(watchlist_repository=repository)
    try:
        app.launcher.build()

        app.open_watchlist()

        assert app.watchlist is not None
        assert dpg.does_item_exist(WATCHLIST_WINDOW)
        app.close_watchlist()
        assert app.watchlist is None
        assert not dpg.does_item_exist(WATCHLIST_WINDOW)
    finally:
        app.bridge.shutdown()
        dpg.destroy_context()


def test_refresh_uses_launcher_paths_and_returns_to_reloaded_watchlist(
    tmp_path: Path,
    monkeypatch,
) -> None:
    repository = _repository(tmp_path)
    finished = Event()
    captured = {}

    def fake_refresh(repository, request, options, **kwargs):
        captured["request"] = request
        captured["options"] = options
        finished.set()

    monkeypatch.setattr("muzik.gui.app.run_watchlist_refresh", fake_refresh)
    dpg.create_context()
    app = MuzikGuiApp(watchlist_repository=repository)
    try:
        app.launcher.build()
        downloads = tmp_path / "custom-downloads"
        splits = tmp_path / "custom-splits"
        dpg.set_value(FIELD_TAGS["output"], str(downloads))
        dpg.set_value(FIELD_TAGS["splits"], str(splits))
        dpg.set_value(FIELD_TAGS["no_organize"], True)
        app.open_watchlist()

        app.refresh_watchlist()

        assert finished.wait(1)
        assert app._worker is not None
        app._worker.join(timeout=1)
        app._poll_worker()
        assert captured["request"].output == downloads
        assert captured["request"].splits == splits
        assert captured["options"].no_organize is True
        assert app.pipeline is None
        assert app.watchlist is not None
        assert dpg.does_item_exist(WATCHLIST_WINDOW)
    finally:
        app.bridge.shutdown()
        dpg.destroy_context()


def test_item_action_saves_state_and_returns_to_watchlist(
    tmp_path: Path,
    monkeypatch,
) -> None:
    repository = _repository(tmp_path, with_item=True)
    ran = Event()

    def operations(**kwargs):
        return ItemActionOperations(
            run_workflow=lambda request, options, cancellation: ran.set(),
            parse_chapters=lambda audio, url, cancellation: audio,
        )

    monkeypatch.setattr("muzik.gui.app.build_item_action_operations", operations)
    dpg.create_context()
    app = MuzikGuiApp(watchlist_repository=repository)
    try:
        app.launcher.build()
        app.open_watchlist()
        item = repository.load().playlists[0].items[0]

        app.request_item_action("PL123", item, ItemAction.RUN)

        assert ran.wait(1)
        assert app._worker is not None
        app._worker.join(timeout=1)
        app._poll_worker()
        saved = repository.load().playlists[0]
        assert saved.processed_video_ids == ["video000001"]
        assert saved.items[0].stages["download"].status.value == "complete"
        assert app.pipeline is None
        assert app.watchlist is not None
    finally:
        app.bridge.shutdown()
        dpg.destroy_context()


def test_back_cancels_refresh_then_returns_to_watchlist(
    tmp_path: Path,
    monkeypatch,
) -> None:
    repository = _repository(tmp_path)
    started = Event()
    stopped = Event()

    def fake_refresh(repository, request, options, **kwargs):
        cancellation = kwargs["cancellation"]
        started.set()
        while not cancellation.is_cancelled():
            time.sleep(0.005)
        stopped.set()
        raise WorkflowCancelled("Workflow cancelled.")

    monkeypatch.setattr("muzik.gui.app.run_watchlist_refresh", fake_refresh)
    dpg.create_context()
    app = MuzikGuiApp(watchlist_repository=repository)
    try:
        app.launcher.build()
        app.open_watchlist()
        app.refresh_watchlist()
        assert started.wait(1)

        app.back()

        assert stopped.wait(1)
        assert app._worker is not None
        app._worker.join(timeout=1)
        app._poll_worker()
        assert app.pipeline is None
        assert app.watchlist is not None
        assert dpg.does_item_exist(WATCHLIST_WINDOW)
    finally:
        app.bridge.shutdown()
        dpg.destroy_context()


def test_cached_texture_load_is_sent_through_bridge(
    tmp_path: Path,
    monkeypatch,
) -> None:
    repository = _repository(tmp_path, with_item=True)
    thumbnail = tmp_path / "thumb.jpg"
    thumbnail.write_bytes(b"cached")
    loaded: list[tuple[str, Path]] = []
    monkeypatch.setattr(
        "muzik.gui.app.cached_thumbnail_path",
        lambda video_id: thumbnail,
    )

    dpg.create_context()
    app = MuzikGuiApp(watchlist_repository=repository)
    try:
        app.launcher.build()
        app.open_watchlist()
        assert app.watchlist is not None
        monkeypatch.setattr(
            app.watchlist,
            "load_cached_thumbnail",
            lambda video_id, path: loaded.append((video_id, path)),
        )

        assert loaded == []
        app.bridge.drain()
        assert loaded == [("video000001", thumbnail)]
    finally:
        app.bridge.shutdown()
        dpg.destroy_context()


def test_overwrite_item_action_starts_only_after_confirmation(
    tmp_path: Path,
    monkeypatch,
) -> None:
    repository = _repository(tmp_path, with_item=True)
    starts: list[ItemAction] = []
    dpg.create_context()
    app = MuzikGuiApp(watchlist_repository=repository)
    try:
        item = repository.load().playlists[0].items[0]
        monkeypatch.setattr(
            app,
            "_start_item_action",
            lambda playlist_id, position, video_id, action, title: starts.append(
                action
            ),
        )

        app.request_item_action("PL123", item, ItemAction.DOWNLOAD_AGAIN)
        assert starts == []

        matching = [
            item_id
            for item_id in dpg.get_all_items()
            if dpg.get_item_label(item_id) == "Download again"
        ]
        assert len(matching) == 1
        callback = dpg.get_item_callback(matching[0])
        assert callback is not None
        callback()
        assert starts == [ItemAction.DOWNLOAD_AGAIN]
    finally:
        app.bridge.shutdown()
        dpg.destroy_context()

from threading import Event
import time
from pathlib import Path
from typing import Any, cast

import dearpygui.dearpygui as dpg

from muzik.core.quality import QualityPolicy
from muzik.core.thumbnails import ThumbnailRequest, ThumbnailResult
from muzik.core.watchlist import (
    StageStatus,
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
from muzik.gui.app import MuzikGuiApp, _workflow_options
from muzik.gui.watchlist import WATCHLIST_ROOT


def test_workflow_options_carries_the_launcher_quality_settings() -> None:
    config = WorkflowLaunchConfig(
        raw="https://youtube.com/watch?v=abcdefghijk",
        quality_policy=QualityPolicy.ASK,
        min_bitrate=192,
    )

    options = _workflow_options(config)

    assert QualityPolicy(options.quality_policy) == QualityPolicy.ASK
    assert options.min_bitrate == 192


def test_direct_gui_launch_sets_viewport_icons(monkeypatch) -> None:
    viewport: dict[str, object] = {}
    app = MuzikGuiApp()
    monkeypatch.setattr(dpg, "create_context", lambda: None)
    monkeypatch.setattr(dpg, "configure_app", lambda **kwargs: None)
    monkeypatch.setattr(
        dpg, "create_viewport", lambda **kwargs: viewport.update(kwargs)
    )
    monkeypatch.setattr(dpg, "setup_dearpygui", lambda: None)
    monkeypatch.setattr(dpg, "show_viewport", lambda: None)
    monkeypatch.setattr(dpg, "set_primary_window", lambda *args: None)
    monkeypatch.setattr(dpg, "is_dearpygui_running", lambda: False)
    monkeypatch.setattr(dpg, "destroy_context", lambda: None)
    monkeypatch.setattr("muzik.gui.app.apply_global_theme", lambda: None)
    monkeypatch.setattr("muzik.gui.app.shell.build", lambda **kwargs: None)

    app.run()

    small_icon = Path(str(viewport["small_icon"]))
    large_icon = Path(str(viewport["large_icon"]))
    assert small_icon.name == "muzik-logo-v2.png"
    assert large_icon == small_icon
    assert small_icon.is_file()


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


def test_watchlist_navigation_selects_tab_and_reloads_saved_data(
    tmp_path: Path,
) -> None:
    repository = _repository(tmp_path, with_item=True)
    dpg.create_context()
    app = MuzikGuiApp(watchlist_repository=repository)
    try:
        app.launcher.build()
        app.watchlist.build()

        app.open_watchlist()

        assert dpg.does_item_exist(WATCHLIST_ROOT)
        assert dpg.does_item_exist("watchlist-row-1")
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
        app.watchlist.build()
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
        assert dpg.does_item_exist(WATCHLIST_ROOT)
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
            check_quality=lambda audio, options, cancellation: (_ for _ in ()).throw(
                AssertionError("check_quality should not run")
            ),
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
        app.watchlist.build()
        app.open_watchlist()
        app.refresh_watchlist()
        assert started.wait(1)

        app.back()

        assert stopped.wait(1)
        assert app._worker is not None
        app._worker.join(timeout=1)
        app._poll_worker()
        assert app.pipeline is None
        assert dpg.does_item_exist(WATCHLIST_ROOT)
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
        app.watchlist.build()
        app.open_watchlist()
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


def test_missing_thumbnail_is_cached_when_item_becomes_visible(
    tmp_path: Path,
    monkeypatch,
) -> None:
    repository = _repository(tmp_path, with_item=True)
    thumbnail = tmp_path / "thumb.jpg"
    thumbnail.write_bytes(b"cached")
    requests: list[ThumbnailRequest] = []
    loaded: list[tuple[str, Path]] = []

    async def fake_cache(values, **kwargs):
        requests.extend(values)
        return [ThumbnailResult("video000001", thumbnail)]

    class FakeView:
        def load_cached_thumbnail(self, video_id: str, path: Path) -> None:
            loaded.append((video_id, path))

    monkeypatch.setattr("muzik.gui.app.cached_thumbnail_path", lambda video_id: None)
    monkeypatch.setattr("muzik.gui.app.cache_thumbnails", fake_cache)
    app = MuzikGuiApp(watchlist_repository=repository)
    app.watchlist = cast(Any, FakeView())
    try:
        app._queue_cached_thumbnail("video000001", "https://example.test/thumb.jpg")
        app._thumbnail_workers["video000001"].join(timeout=1)
        app.bridge.drain()

        assert requests == [
            ThumbnailRequest("video000001", "https://example.test/thumb.jpg")
        ]
        assert loaded == [("video000001", thumbnail)]
    finally:
        app._thumbnail_cancellation.cancel()
        app.bridge.shutdown()


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


def test_spotify_window_shows_the_saved_client_id_and_adds_a_source(
    tmp_path: Path,
    monkeypatch,
) -> None:
    from muzik.core.sources.spotify_auth import TokenStore
    from muzik.gui import app as gui_app
    from muzik.gui.spotify import CLIENT_ID_INPUT, SPOTIFY_BODY

    config = tmp_path / "config.yaml"
    monkeypatch.setattr(
        gui_app,
        "get_spotify_settings",
        lambda: {"client_id": "client-1", "redirect_port": "8888"},
    )
    monkeypatch.setattr(
        gui_app, "TokenStore", lambda: TokenStore(tmp_path / "no-token.json")
    )
    monkeypatch.setattr(
        gui_app,
        "save_muzik_config_value",
        lambda section, key, value: config.write_text(f"{section}.{key}={value}"),
    )
    repository = WatchlistRepository(tmp_path / "watchlist.json")
    dpg.create_context()
    app = MuzikGuiApp(watchlist_repository=repository)
    try:
        app.launcher.build()
        app.watchlist.build()

        app.open_spotify_dialog()

        assert dpg.does_item_exist(SPOTIFY_BODY)
        assert dpg.get_value(CLIENT_ID_INPUT) == "client-1"
        assert app.spotify.is_open is True

        app.save_spotify_client_id("client-2")
        app.add_watchlist_playlist("spotify:liked")

        assert config.read_text() == "spotify.client_id=client-2"
        assert [playlist.playlist_id for playlist in repository.load().playlists] == [
            "spotify:liked"
        ]
    finally:
        app.spotify.close()
        app.bridge.shutdown()
        dpg.destroy_context()


def test_reload_renders_at_once_and_reconciles_in_a_worker(tmp_path: Path) -> None:
    """The render thread must never wait for Beets and the download folder."""
    started = Event()
    release = Event()
    reconciled: list[str] = []

    def slow_reconcile(watchlist, *, request, options) -> None:
        started.set()
        release.wait(5)
        for playlist in watchlist.playlists:
            reconciled.append(playlist.playlist_id)
            for item in playlist.items:
                item.stages["download"].status = StageStatus.COMPLETE

    repository = _repository(tmp_path, with_item=True)
    dpg.create_context()
    app = MuzikGuiApp(watchlist_repository=repository, reconcile=slow_reconcile)
    try:
        app.launcher.build()
        app.watchlist.build()

        app.open_watchlist()

        # The cards are on screen before the reconcile has even finished.
        assert dpg.does_item_exist("watchlist-row-1")
        assert started.wait(5) is True
        assert dpg.get_value("watchlist-status") == "Checking local files and Beets..."

        release.set()
        for _ in range(200):
            app.bridge.drain()
            if reconciled and dpg.get_value("watchlist-status") == "":
                break
            time.sleep(0.01)

        assert reconciled == ["PL123"]
        assert dpg.get_value("watchlist-status") == ""
        saved = repository.load().playlists[0].items[0]
        assert saved.stages["download"].status is StageStatus.COMPLETE
    finally:
        app.bridge.shutdown()
        dpg.destroy_context()


def test_a_watchlist_change_during_a_reconcile_is_not_overwritten(
    tmp_path: Path,
) -> None:
    release = Event()

    def slow_reconcile(watchlist, *, request, options) -> None:
        release.wait(5)

    repository = _repository(tmp_path, with_item=True)
    dpg.create_context()
    app = MuzikGuiApp(watchlist_repository=repository, reconcile=slow_reconcile)
    try:
        app.launcher.build()
        app.watchlist.build()
        app.open_watchlist()

        app.add_watchlist_playlist("https://youtube.com/playlist?list=PL_NEW")
        release.set()
        for _ in range(200):
            app.bridge.drain()
            time.sleep(0.01)
            worker = app._reconcile_worker
            if worker is None or not worker.is_alive():
                break

        assert [playlist.playlist_id for playlist in repository.load().playlists] == [
            "PL123",
            "PL_NEW",
        ]
    finally:
        app.bridge.shutdown()
        dpg.destroy_context()

"""DearPyGui application entry point and workflow runner."""

from __future__ import annotations

import asyncio
from collections.abc import Callable
import inspect
from pathlib import Path
import sys
from threading import Thread
from typing import Any
import webbrowser

import dearpygui.dearpygui as dpg

from muzik.branding import logo_path
from muzik.core.workflow.cancellation import CancellationToken, WorkflowCancelled
from muzik.core.workflow.decisions import WorkflowDecisions
from muzik.core.workflow.events import ErrorEvent, MessageEvent, WorkflowEventEmitter
from muzik.core.workflow.launch import WorkflowLaunchConfig
from muzik.core.workflow.operations import build_workflow_operations
from muzik.core.workflow.operations import build_item_action_operations
from muzik.core.workflow.item_actions import (
    ItemAction,
    item_summary_state,
    run_item_action,
)
from muzik.core.workflow.service import (
    WorkflowOptions,
    WorkflowRequest,
    WorkflowRunOperations,
    WorkflowServiceError,
    run_workflow,
)
from muzik.gui import modals
from muzik.gui.adapters import (
    GuiBeetsDecisions,
    GuiBeetsEventEmitter,
    GuiWorkflowDecisions,
    GuiWorkflowEventEmitter,
)
from muzik.core.services import check_services
from muzik.config import (
    DEFAULT_DOWNLOAD_DIR,
    get_spotify_settings,
    save_muzik_config_value,
)
from muzik.core.sources.spotify_api import (
    SpotifyApiError,
    SpotifyClient,
    SpotifyPlaylistRef,
)
from muzik.core.sources.spotify_auth import (
    SpotifyAuthError,
    TokenStore,
    login as spotify_login,
    redirect_uri as spotify_redirect_uri,
)
from muzik.core.library import scan_downloads
from muzik.gui.bridge import GuiBridge
from muzik.gui import shell
from muzik.gui.launcher import LauncherView
from muzik.gui.library import LibraryView
from muzik.gui.pipeline import PipelineView
from muzik.gui.settings import SettingsView
from muzik.gui.theme import apply_global_theme
from muzik.core.thumbnails import (
    ThumbnailRequest,
    ThumbnailResult,
    cache_thumbnails,
    cached_thumbnail_path,
)
from muzik.core.watchlist import (
    WatchlistError,
    WatchlistItem,
    WatchlistRepository,
    reconcile_watchlist,
    refresh_watchlist as run_watchlist_refresh,
)
from muzik.gui.spotify import SpotifyDialog, SpotifyState
from muzik.gui.watchlist import WATCHLIST_ROOT, WatchlistView


WorkflowOperationsFactory = Callable[..., WorkflowRunOperations]
WatchlistReconciler = Callable[..., None]


class MuzikGuiApp:
    """Own the desktop render loop and one background workflow at a time."""

    def __init__(
        self,
        *,
        operations_factory: WorkflowOperationsFactory | None = None,
        watchlist_repository: WatchlistRepository | None = None,
        reconcile: WatchlistReconciler | None = None,
    ) -> None:
        self.operations_factory = operations_factory or _default_operations
        self.watchlist_repository = watchlist_repository or WatchlistRepository()
        self.reconcile = reconcile or reconcile_watchlist
        self.bridge = GuiBridge(on_error=self._handle_bridge_error)
        self.launcher = LauncherView(self.open_pipeline, self.quit)
        self.watchlist = WatchlistView(
            on_add=self.add_watchlist_playlist,
            on_remove=self.remove_watchlist_playlist,
            on_refresh=self.refresh_watchlist,
            on_action=self.request_item_action,
            on_rename=self.rename_watchlist_playlist,
            on_thumbnail_needed=self._queue_cached_thumbnail,
            on_spotify=self.open_spotify_dialog,
            on_back=self._go_to_workflow_tab,
        )
        self.spotify = SpotifyDialog(
            on_save_client_id=self.save_spotify_client_id,
            on_connect=self.connect_spotify,
            on_disconnect=self.disconnect_spotify,
            on_reload=self.reload_spotify_playlists,
            on_add=self.add_watchlist_playlist,
            on_copy=dpg.set_clipboard_text,
            on_open_link=webbrowser.open,
        )
        self._reconcile_worker: Thread | None = None
        self._spotify_worker: Thread | None = None
        self._spotify_playlists: tuple[SpotifyPlaylistRef, ...] = ()
        self._spotify_account = ""
        self.library = LibraryView(
            DEFAULT_DOWNLOAD_DIR,
            self.refresh_library,
            self._go_to_workflow_tab,
        )
        self.settings = SettingsView(self.recheck_services, self._go_to_workflow_tab)
        self.pipeline: PipelineView | None = None
        self._worker: Thread | None = None
        self._settings_worker: Thread | None = None
        self._library_worker: Thread | None = None
        self._thumbnail_workers: dict[str, Thread] = {}
        self._thumbnail_cancellation = CancellationToken()
        self._cancellation: CancellationToken | None = None
        self._worker_is_watchlist_job = False
        self._return_after_worker = False
        self._worker_label = "Workflow"
        self._worker_error: Exception | None = None

    def run(self) -> None:
        """Create the viewport and run callbacks on the main render thread."""
        dpg.create_context()
        try:
            dpg.configure_app(manual_callback_management=True)
            apply_global_theme()
            shell.build(
                launcher=self.launcher,
                watchlist=self.watchlist,
                library=self.library,
                settings=self.settings,
                on_tab_changed=self._on_tab_changed,
                on_quit=self.quit,
            )
            icon = _viewport_icon_path()
            icon_value = str(icon) if icon is not None else ""
            dpg.create_viewport(
                title="muzik",
                small_icon=icon_value,
                large_icon=icon_value,
                width=1280,
                height=800,
            )
            dpg.setup_dearpygui()
            dpg.show_viewport()
            while dpg.is_dearpygui_running():
                dpg.run_callbacks(dpg.get_callback_queue())
                self.bridge.drain()
                self._poll_worker()
                if dpg.does_item_exist(WATCHLIST_ROOT):
                    self.watchlist.set_available_width(dpg.get_viewport_client_width())
                dpg.render_dearpygui_frame()
        finally:
            self._cancel_worker()
            self._thumbnail_cancellation.cancel()
            self.bridge.shutdown()
            if self._worker is not None:
                self._worker.join(timeout=5)
            if self._settings_worker is not None:
                self._settings_worker.join(timeout=5)
            if self._library_worker is not None:
                self._library_worker.join(timeout=5)
            if self._reconcile_worker is not None:
                self._reconcile_worker.join(timeout=5)
            if self._spotify_worker is not None:
                self._spotify_worker.join(timeout=1)
            for worker in tuple(self._thumbnail_workers.values()):
                worker.join(timeout=1)
            dpg.destroy_context()

    def open_pipeline(self, config: WorkflowLaunchConfig) -> None:
        """Build the pipeline modal and start its workflow worker."""
        if self._worker is not None and self._worker.is_alive():
            return
        self._worker_is_watchlist_job = False
        self._return_after_worker = False
        self._worker_label = "Workflow"
        self._worker_error = None
        self._cancellation = CancellationToken()
        self.pipeline = PipelineView(self.back, self.quit)
        self.pipeline.build(config.raw)
        self._worker = Thread(
            target=self._run_workflow,
            args=(config,),
            name="muzik-workflow",
            daemon=True,
        )
        self._worker.start()

    def back(
        self,
        sender: Any = None,
        app_data: Any = None,
        user_data: Any = None,
    ) -> None:
        """Cancel an active run, or dismiss a finished pipeline modal."""
        if self._worker is not None and self._worker.is_alive():
            self._return_after_worker = True
            self._cancel_worker()
            modals.close_all_modals()
            if self.pipeline is not None:
                self.pipeline.set_status("Cancelling...")
                self.pipeline.disable_back()
            return
        self._close_pipeline()

    def _go_to_workflow_tab(
        self,
        sender: Any = None,
        app_data: Any = None,
        user_data: Any = None,
    ) -> None:
        if dpg.does_item_exist(shell.NAV_TABS):
            dpg.set_value(shell.NAV_TABS, shell.TAB_WORKFLOW)

    def _on_tab_changed(self, tab: str) -> None:
        """Load the newly selected tab's data (mirrors what opening it did)."""
        if tab == shell.TAB_WATCHLIST:
            self._reload_watchlist()
        elif tab == shell.TAB_LIBRARY:
            self._start_library_scan()
        elif tab == shell.TAB_SETTINGS:
            self._start_service_checks()

    def open_watchlist(
        self,
        sender: Any = None,
        app_data: Any = None,
        user_data: Any = None,
    ) -> None:
        """Select the Watchlist tab and reload its saved data."""
        if self._worker is not None and self._worker.is_alive():
            return
        if dpg.does_item_exist(shell.NAV_TABS):
            dpg.set_value(shell.NAV_TABS, shell.TAB_WATCHLIST)
        self._reload_watchlist()

    def add_watchlist_playlist(self, url: str) -> None:
        """Add one normalized source and reload the rail."""
        try:
            self.watchlist_repository.add(url)
        except WatchlistError as exc:
            if self.spotify.is_open:
                self.spotify.show(self._spotify_state(error=str(exc)))
                return
            if self.watchlist is not None:
                self.watchlist.show_error(str(exc))
            return
        if self.watchlist is not None:
            self.watchlist.clear_error()
        self._reload_watchlist()
        if self.spotify.is_open:
            self.spotify.show(
                self._spotify_state(status="Added. Select Refresh to sync it.")
            )

    def open_spotify_dialog(
        self,
        sender: Any = None,
        app_data: Any = None,
        user_data: Any = None,
    ) -> None:
        """Show the Spotify window and read the account in the background."""
        self.spotify.open(self._spotify_state())
        if TokenStore().load() is not None:
            self._start_spotify_worker(self._load_spotify_account)

    def save_spotify_client_id(self, value: str) -> None:
        """Save the client ID of the user's own Spotify application."""
        try:
            save_muzik_config_value("spotify", "client_id", value)
        except OSError as exc:
            self.spotify.show(self._spotify_state(error=str(exc)))
            return
        self.spotify.show(self._spotify_state(status="Client ID saved."))

    def connect_spotify(self) -> None:
        """Run the Spotify login in a browser, from a worker thread."""
        self.spotify.show(
            self._spotify_state(status="Approve muzik in your browser.", busy=True)
        )
        self._start_spotify_worker(self._run_spotify_login)

    def disconnect_spotify(self) -> None:
        """Remove the saved Spotify tokens."""
        TokenStore().clear()
        self._spotify_playlists = ()
        self.spotify.show(self._spotify_state(status="Spotify is disconnected."))

    def reload_spotify_playlists(self) -> None:
        """Read the playlists of the connected account again."""
        self._spotify_playlists = ()
        self.spotify.show(self._spotify_state(busy=True))
        self._start_spotify_worker(self._load_spotify_account)

    def _spotify_state(
        self,
        *,
        account: str = "",
        status: str = "",
        error: str = "",
        busy: bool = False,
    ) -> SpotifyState:
        settings = get_spotify_settings()
        try:
            saved = {
                playlist.playlist_id
                for playlist in self.watchlist_repository.load().playlists
            }
        except WatchlistError:
            saved = set()
        return SpotifyState(
            client_id=settings.get("client_id", ""),
            redirect_uri=spotify_redirect_uri(
                int(settings.get("redirect_port", "8888") or "8888")
            ),
            connected=TokenStore().load() is not None,
            account=account or self._spotify_account,
            playlists=self._spotify_playlists,
            saved_uris=frozenset(saved),
            status=status,
            error=error,
            busy=busy,
        )

    def _start_spotify_worker(self, target: Callable[[], None]) -> None:
        if self._spotify_worker is not None and self._spotify_worker.is_alive():
            return
        self._spotify_worker = Thread(
            target=target,
            name="muzik-spotify",
            daemon=True,
        )
        self._spotify_worker.start()

    def _run_spotify_login(self) -> None:
        try:
            spotify_login()
        except (SpotifyApiError, SpotifyAuthError) as exc:
            self._submit_spotify_state(error=str(exc))
            return
        self._load_spotify_account()

    def _load_spotify_account(self) -> None:
        try:
            client = SpotifyClient()
            account = client.account_name()
            playlists = tuple(client.list_playlists())
        except (SpotifyApiError, SpotifyAuthError) as exc:
            self._submit_spotify_state(error=str(exc))
            return
        self._spotify_account = account
        self._spotify_playlists = playlists
        self._submit_spotify_state(account=account)

    def _submit_spotify_state(self, *, account: str = "", error: str = "") -> None:
        if self.bridge.is_shutdown():
            return

        def update() -> None:
            if self.spotify.is_open:
                self.spotify.show(self._spotify_state(account=account, error=error))

        self.bridge.submit(update)

    def rename_watchlist_playlist(self, playlist_id: str, title: str) -> None:
        """Give one saved source the name that the user entered."""
        try:
            self.watchlist_repository.rename(playlist_id, title)
        except WatchlistError as exc:
            if self.watchlist is not None:
                self.watchlist.show_error(str(exc))
            return
        self._reload_watchlist()

    def remove_watchlist_playlist(self, playlist_id: str) -> None:
        """Remove one playlist from the watchlist."""
        try:
            self.watchlist_repository.remove(playlist_id)
        except WatchlistError as exc:
            if self.watchlist is not None:
                self.watchlist.show_error(str(exc))
            return
        self._reload_watchlist()

    def refresh_watchlist(
        self,
        sender: Any = None,
        app_data: Any = None,
        user_data: Any = None,
    ) -> None:
        """Start one worker that checks all saved playlists."""
        if self._worker is not None and self._worker.is_alive():
            return
        try:
            watchlist = self.watchlist_repository.load()
            config = self.launcher.read_config()
        except (WatchlistError, TypeError, ValueError) as exc:
            if self.watchlist is not None:
                self.watchlist.show_error(str(exc))
            return
        if not watchlist.playlists:
            if self.watchlist is not None:
                self.watchlist.show_error("Add a playlist before you refresh.")
            return
        self._start_watchlist_worker(
            config,
            target=self._run_watchlist_refresh,
            args=(config,),
            label="Watchlist refresh",
            raw="Check playlists for new videos",
        )

    def request_item_action(
        self,
        playlist_id: str,
        item: WatchlistItem,
        action: ItemAction,
    ) -> None:
        """Confirm overwrite commands, then start one item worker."""
        force_actions = {
            ItemAction.DOWNLOAD_AGAIN,
            ItemAction.PARSE_AGAIN,
            ItemAction.SPLIT_AGAIN,
            ItemAction.ORGANIZE_AGAIN,
            ItemAction.RUN_ALL_AGAIN,
        }
        if action in force_actions:
            label = action.value.replace("_", " ").capitalize()
            modals.confirm_item_action_modal(
                item_title=item.title,
                action_label=label,
                on_confirm=lambda: self._start_item_action(
                    playlist_id,
                    item.position,
                    item.video_id,
                    action,
                    item.title,
                ),
            )
            return
        self._start_item_action(
            playlist_id,
            item.position,
            item.video_id,
            action,
            item.title,
        )

    def _start_item_action(
        self,
        playlist_id: str,
        position: int,
        video_id: str | None,
        action: ItemAction,
        title: str,
    ) -> None:
        if self._worker is not None and self._worker.is_alive():
            return
        try:
            config = self.launcher.read_config()
        except (TypeError, ValueError) as exc:
            if self.watchlist is not None:
                self.watchlist.show_error(str(exc))
            return
        label = action.value.replace("_", " ").capitalize()
        self._start_watchlist_worker(
            config,
            target=self._run_item_action,
            args=(config, playlist_id, position, video_id, action),
            label=label,
            raw=f"{label}: {title}",
        )

    def _start_watchlist_worker(
        self,
        config: WorkflowLaunchConfig,
        *,
        target: Callable[..., None],
        args: tuple[Any, ...],
        label: str,
        raw: str,
    ) -> None:
        self._worker_is_watchlist_job = True
        self._return_after_worker = False
        self._worker_label = label
        self._worker_error = None
        self._cancellation = CancellationToken()
        self.pipeline = PipelineView(self.back, self.quit)
        self.pipeline.build(raw)
        self._worker = Thread(
            target=target,
            args=args,
            name="muzik-watchlist",
            daemon=True,
        )
        self._worker.start()

    def quit(
        self,
        sender: Any = None,
        app_data: Any = None,
        user_data: Any = None,
    ) -> None:
        """Cancel active work and stop the viewport."""
        self._cancel_worker()
        modals.close_all_modals()
        if self.watchlist is not None:
            self.watchlist.release_textures()
        dpg.stop_dearpygui()

    def open_settings(
        self,
        sender: Any = None,
        app_data: Any = None,
        user_data: Any = None,
    ) -> None:
        """Select the Settings tab and start the service checks."""
        if dpg.does_item_exist(shell.NAV_TABS):
            dpg.set_value(shell.NAV_TABS, shell.TAB_SETTINGS)
        self._start_service_checks()

    def recheck_services(
        self,
        sender: Any = None,
        app_data: Any = None,
        user_data: Any = None,
    ) -> None:
        """Re-run the service checks for the Settings tab."""
        self._start_service_checks()

    def _start_service_checks(self) -> None:
        if self._settings_worker is not None and self._settings_worker.is_alive():
            return
        if self.settings is not None:
            self.settings.set_checking()
        self._settings_worker = Thread(
            target=self._run_service_checks,
            name="muzik-service-checks",
            daemon=True,
        )
        self._settings_worker.start()

    def _run_service_checks(self) -> None:
        statuses = check_services()
        if self.bridge.is_shutdown():
            return

        def update() -> None:
            if self.settings is not None:
                self.settings.load_statuses(statuses)

        self.bridge.submit(update)

    def open_library(
        self,
        sender: Any = None,
        app_data: Any = None,
        user_data: Any = None,
    ) -> None:
        """Select the Library tab and scan the output folder."""
        if self._worker is not None and self._worker.is_alive():
            return
        if dpg.does_item_exist(shell.NAV_TABS):
            dpg.set_value(shell.NAV_TABS, shell.TAB_LIBRARY)
        self._start_library_scan()

    def refresh_library(
        self,
        sender: Any = None,
        app_data: Any = None,
        user_data: Any = None,
    ) -> None:
        """Re-scan the output folder for the Library tab."""
        self._start_library_scan()

    def _start_library_scan(self) -> None:
        if self._library_worker is not None and self._library_worker.is_alive():
            return
        if self.library is not None:
            self.library.set_scanning()
        self._library_worker = Thread(
            target=self._run_library_scan,
            name="muzik-library-scan",
            daemon=True,
        )
        self._library_worker.start()

    def _run_library_scan(self) -> None:
        items = scan_downloads(DEFAULT_DOWNLOAD_DIR)
        if self.bridge.is_shutdown():
            return

        def update() -> None:
            if self.library is not None:
                self.library.load_items(items)

        self.bridge.submit(update)

    def _run_workflow(self, config: WorkflowLaunchConfig) -> None:
        cancellation = self._cancellation
        pipeline = self.pipeline
        if cancellation is None or pipeline is None:
            return
        request = WorkflowRequest(
            raw=config.raw,
            output=config.output,
            splits=config.splits,
        )
        options = _workflow_options(config)
        events = GuiWorkflowEventEmitter(self.bridge, pipeline, cancellation)
        beets_events = GuiBeetsEventEmitter(self.bridge, pipeline, cancellation)
        beets_decisions = GuiBeetsDecisions(
            self.bridge,
            interactive=config.interactive,
            cancellation=cancellation,
            match_presenter=pipeline.request_beets_match,
        )
        decisions = GuiWorkflowDecisions(
            self.bridge,
            interactive=config.interactive,
            cancellation=cancellation,
        )
        try:
            operations = self._make_operations(
                config,
                decisions,
                events,
                beets_decisions,
                beets_events,
            )
            run_workflow(
                request,
                options,
                operations=operations,
                events=events,
                cancellation=cancellation,
            )
        except WorkflowCancelled:
            return
        except WorkflowServiceError as exc:
            self._worker_error = exc
            events.emit(ErrorEvent(exc.message, fatal=True))
        except Exception as exc:
            self._worker_error = exc
            events.emit(ErrorEvent(str(exc), fatal=True))

    def _run_watchlist_refresh(self, config: WorkflowLaunchConfig) -> None:
        cancellation = self._cancellation
        pipeline = self.pipeline
        if cancellation is None or pipeline is None:
            return
        request = WorkflowRequest("", config.output, config.splits)
        options = _workflow_options(config)
        events = GuiWorkflowEventEmitter(self.bridge, pipeline, cancellation)
        beets_events = GuiBeetsEventEmitter(self.bridge, pipeline, cancellation)
        beets_decisions = GuiBeetsDecisions(
            self.bridge,
            interactive=config.interactive,
            cancellation=cancellation,
            match_presenter=pipeline.request_beets_match,
        )
        decisions = GuiWorkflowDecisions(
            self.bridge,
            interactive=config.interactive,
            cancellation=cancellation,
        )
        try:
            operations = self._make_operations(
                config,
                decisions,
                events,
                beets_decisions,
                beets_events,
            )
            run_watchlist_refresh(
                self.watchlist_repository,
                request,
                options,
                operations=operations,
                events=events,
                cancellation=cancellation,
            )
            self._cache_watchlist_thumbnails(cancellation, events)
        except WorkflowCancelled:
            return
        except Exception as exc:
            self._worker_error = exc
            events.emit(ErrorEvent(str(exc), fatal=True))

    def _run_item_action(
        self,
        config: WorkflowLaunchConfig,
        playlist_id: str,
        position: int,
        video_id: str | None,
        action: ItemAction,
    ) -> None:
        cancellation = self._cancellation
        pipeline = self.pipeline
        if cancellation is None or pipeline is None:
            return
        request = WorkflowRequest("", config.output, config.splits)
        options = _workflow_options(config)
        events = GuiWorkflowEventEmitter(self.bridge, pipeline, cancellation)
        beets_events = GuiBeetsEventEmitter(self.bridge, pipeline, cancellation)
        beets_decisions = GuiBeetsDecisions(
            self.bridge,
            interactive=config.interactive,
            cancellation=cancellation,
            match_presenter=pipeline.request_beets_match,
        )
        decisions = GuiWorkflowDecisions(
            self.bridge,
            interactive=config.interactive,
            cancellation=cancellation,
        )
        try:
            watchlist = self.watchlist_repository.load()
            playlist = next(
                (
                    entry
                    for entry in watchlist.playlists
                    if entry.playlist_id == playlist_id
                ),
                None,
            )
            if playlist is None:
                raise WatchlistError("The selected playlist is no longer available.")
            item = next(
                (
                    entry
                    for entry in playlist.items
                    if entry.position == position and entry.video_id == video_id
                ),
                None,
            )
            if item is None:
                raise WatchlistError("The selected video is no longer available.")
            operations = build_item_action_operations(
                decisions=decisions,
                events=events,
                beets_decisions=beets_decisions,
                beets_events=beets_events,
            )
            run_item_action(
                item,
                action,
                request=request,
                options=options,
                operations=operations,
                source_id=playlist.playlist_id,
                cancellation=cancellation,
                on_state_change=lambda: self.watchlist_repository.save(watchlist),
            )
            done_key = item.entry_id or item.video_id
            if (
                done_key
                and item_summary_state(item) == "Processed"
                and done_key not in playlist.processed_video_ids
            ):
                playlist.processed_video_ids.append(done_key)
            self.watchlist_repository.save(watchlist)
        except WorkflowCancelled:
            return
        except Exception as exc:
            self._worker_error = exc
            events.emit(ErrorEvent(str(exc), fatal=True))

    def _cache_watchlist_thumbnails(
        self,
        cancellation: CancellationToken,
        events: GuiWorkflowEventEmitter,
    ) -> None:
        watchlist = self.watchlist_repository.load()
        requests: dict[str, ThumbnailRequest] = {}
        for playlist in watchlist.playlists:
            for item in playlist.items:
                if (
                    item.video_id
                    and item.thumbnail_url
                    and cached_thumbnail_path(item.video_id) is None
                ):
                    requests[item.video_id] = ThumbnailRequest(
                        item.video_id,
                        item.thumbnail_url,
                    )
        if not requests:
            return
        events.emit(MessageEvent(f"Caching {len(requests)} thumbnail(s)."))
        results = asyncio.run(
            cache_thumbnails(requests.values(), cancellation=cancellation)
        )
        failed = [result for result in results if result.error]
        if failed:
            events.emit(
                MessageEvent(
                    f"{len(failed)} thumbnail(s) could not be cached. "
                    "The app will try them on the next refresh.",
                    severity="warning",
                )
            )

    def _make_operations(
        self,
        config: WorkflowLaunchConfig,
        decisions: GuiWorkflowDecisions,
        events: GuiWorkflowEventEmitter,
        beets_decisions: GuiBeetsDecisions,
        beets_events: GuiBeetsEventEmitter,
    ) -> WorkflowRunOperations:
        parameters = inspect.signature(self.operations_factory).parameters.values()
        supports_beets_adapters = (
            any(
                parameter.kind
                in {inspect.Parameter.VAR_POSITIONAL, inspect.Parameter.VAR_KEYWORD}
                for parameter in parameters
            )
            or len(inspect.signature(self.operations_factory).parameters) >= 5
        )
        if supports_beets_adapters:
            return self.operations_factory(
                config,
                decisions,
                events,
                beets_decisions,
                beets_events,
            )
        return self.operations_factory(config, decisions, events)

    def _cancel_worker(self) -> None:
        if self._cancellation is not None:
            self._cancellation.cancel()
        self.bridge.cancel_pending()

    def _handle_bridge_error(self, error: Exception) -> None:
        if self.pipeline is not None:
            self.pipeline.log(f"Interface update failed: {error}")

    def _poll_worker(self) -> None:
        if self._worker is None or self._worker.is_alive():
            return
        self._worker.join()
        if self._return_after_worker or self._worker_is_watchlist_job:
            self._close_pipeline()
            return
        if self.pipeline is not None:
            self.pipeline.set_busy(False)
            if self._worker_error is None:
                self.pipeline.set_status("Complete")
                self.pipeline.log(f"{self._worker_label} complete.")
            else:
                self.pipeline.set_status("Failed")
                self.pipeline.log(f"{self._worker_label} failed: {self._worker_error}")
        self._worker = None

    def _close_pipeline(self) -> None:
        """Dismiss the pipeline modal; reload the watchlist if it ran the job.

        The tab bar is never touched here — unlike the old window-swapping
        design, the tab the user was on (Workflow or Watchlist) never left
        the screen while the pipeline modal was open on top of it.
        """
        error = self._worker_error
        if self.pipeline is not None:
            self.pipeline.destroy()
        self.pipeline = None
        self._worker = None
        self._cancellation = None
        self._return_after_worker = False
        was_watchlist_job = self._worker_is_watchlist_job
        self._worker_is_watchlist_job = False
        self._worker_error = None
        if was_watchlist_job:
            self._reload_watchlist()
            if error is not None:
                self.watchlist.show_error(str(error))

    def _reload_watchlist(self) -> None:
        """Show the saved watchlist at once, then reconcile it in a worker.

        Reconciliation reads the download folder and queries Beets for every
        item, which takes seconds on a large library. It must never run on the
        render thread: that is what froze the whole window.
        """
        view = self.watchlist
        if view is None:
            return
        try:
            config = self.launcher.read_config()
            request = WorkflowRequest("", config.output, config.splits)
            options = _workflow_options(config)
            watchlist = self.watchlist_repository.load()
        except (WatchlistError, TypeError, ValueError) as exc:
            view.show_error(str(exc))
            return
        view.load(watchlist, request)
        self._start_reconcile(request, options)

    def _watchlist_stamp(self) -> tuple[int, int]:
        """Return a value that changes when the watchlist file is written."""
        try:
            stat = self.watchlist_repository.path.stat()
        except OSError:
            return (0, 0)
        return (stat.st_mtime_ns, stat.st_size)

    def _start_reconcile(
        self,
        request: WorkflowRequest,
        options: WorkflowOptions,
    ) -> None:
        if self._reconcile_worker is not None and self._reconcile_worker.is_alive():
            return
        if self.watchlist is not None:
            self.watchlist.set_status("Checking local files and Beets...")
        self._reconcile_worker = Thread(
            target=self._run_reconcile,
            args=(request, options, self._watchlist_stamp()),
            name="muzik-reconcile",
            daemon=True,
        )
        self._reconcile_worker.start()

    def _run_reconcile(
        self,
        request: WorkflowRequest,
        options: WorkflowOptions,
        stamp: tuple[int, int],
    ) -> None:
        try:
            watchlist = self.watchlist_repository.load()
            self.reconcile(watchlist, request=request, options=options)
        except Exception as exc:
            message = str(exc)

            def report() -> None:
                if self.watchlist is not None:
                    self.watchlist.set_status("")
                    self.watchlist.show_error(message)

            self.bridge.submit(report)
            return

        def apply() -> None:
            view = self.watchlist
            if view is None:
                return
            view.set_status("")
            if self._watchlist_stamp() != stamp:
                # The user added, renamed, or removed a source while this ran.
                # Saving now would drop that change, so start again instead.
                self._start_reconcile(request, options)
                return
            try:
                self.watchlist_repository.save(watchlist)
            except WatchlistError as exc:
                view.show_error(str(exc))
                return
            view.load(watchlist, request, keep_page=True)

        self.bridge.submit(apply)

    def _queue_cached_thumbnail(self, video_id: str, thumbnail_url: str) -> None:
        view = self.watchlist
        path = cached_thumbnail_path(video_id)
        if view is None:
            return
        if path is None:
            self._start_thumbnail_download(view, video_id, thumbnail_url)
            return

        def load_thumbnail() -> None:
            if self.watchlist is view:
                view.load_cached_thumbnail(video_id, path)

        self.bridge.submit(load_thumbnail)

    def _start_thumbnail_download(
        self,
        view: WatchlistView,
        video_id: str,
        thumbnail_url: str,
    ) -> None:
        worker = self._thumbnail_workers.get(video_id)
        if worker is not None and worker.is_alive():
            return
        if not thumbnail_url:
            return

        def download() -> None:
            try:
                results = asyncio.run(
                    cache_thumbnails(
                        [ThumbnailRequest(video_id, thumbnail_url)],
                        cancellation=self._thumbnail_cancellation,
                    )
                )
            except WorkflowCancelled:
                return
            result = results[0] if results else ThumbnailResult(video_id, None)

            def finished() -> None:
                self._thumbnail_workers.pop(video_id, None)
                if self.watchlist is view and result.path is not None:
                    view.load_cached_thumbnail(video_id, result.path)

            self.bridge.submit(finished)

        worker = Thread(
            target=download,
            name=f"muzik-thumbnail-{video_id}",
            daemon=True,
        )
        self._thumbnail_workers[video_id] = worker
        worker.start()


def _workflow_options(config: WorkflowLaunchConfig) -> WorkflowOptions:
    return WorkflowOptions(
        review=config.review,
        no_split=config.no_split,
        no_organize=config.no_organize,
        import_=config.import_,
        tag_only=config.tag_only,
        dry_run=config.dry_run,
        jobs=config.jobs,
        config=config.config,
        keep_source=config.keep_source,
        force=config.force,
        metadata_source=config.metadata_source,
        audio_source=config.audio_source,
        prefer=config.prefer,
        fallback=config.fallback,
        interactive=config.interactive,
        quality_policy=config.quality_policy,
        min_bitrate=config.min_bitrate,
    )


def _default_operations(
    config: WorkflowLaunchConfig,
    decisions: WorkflowDecisions,
    events: WorkflowEventEmitter,
    beets_decisions: GuiBeetsDecisions | None = None,
    beets_events: GuiBeetsEventEmitter | None = None,
) -> WorkflowRunOperations:
    return build_workflow_operations(
        splits=config.splits,
        options=_workflow_options(config),
        decisions=decisions,
        events=events,
        beets_decisions=beets_decisions,
        beets_events=beets_events,
    )


def gui_cmd() -> None:
    """Open the DearPyGui workflow interface."""
    MuzikGuiApp().run()


def _viewport_icon_path() -> Path | None:
    """Return a DearPyGui-compatible logo for direct GUI launches."""
    if sys.platform == "win32":
        return None
    return logo_path()

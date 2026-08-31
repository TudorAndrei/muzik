"""DearPyGui application entry point and workflow runner."""

from __future__ import annotations

import asyncio
from collections.abc import Callable
import inspect
from threading import Thread
from typing import Any

import dearpygui.dearpygui as dpg

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
from muzik.config import DEFAULT_DOWNLOAD_DIR
from muzik.core.library import scan_downloads
from muzik.gui.bridge import GuiBridge
from muzik.gui.launcher import LAUNCHER_WINDOW, LauncherView
from muzik.gui.library import LIBRARY_WINDOW, LibraryView
from muzik.gui.pipeline import PipelineView
from muzik.gui.settings import SETTINGS_WINDOW, SettingsView
from muzik.gui.theme import apply_global_theme
from muzik.core.thumbnails import (
    ThumbnailRequest,
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
from muzik.gui.watchlist import WATCHLIST_WINDOW, WatchlistView


WorkflowOperationsFactory = Callable[..., WorkflowRunOperations]


class MuzikGuiApp:
    """Own the desktop render loop and one background workflow at a time."""

    def __init__(
        self,
        *,
        operations_factory: WorkflowOperationsFactory | None = None,
        watchlist_repository: WatchlistRepository | None = None,
    ) -> None:
        self.operations_factory = operations_factory or _default_operations
        self.watchlist_repository = watchlist_repository or WatchlistRepository()
        self.bridge = GuiBridge(on_error=self._handle_bridge_error)
        self.launcher = LauncherView(
            self.open_pipeline,
            self.quit,
            self.open_settings,
            self.open_library,
            self.open_watchlist,
        )
        self.pipeline: PipelineView | None = None
        self.settings: SettingsView | None = None
        self.library: LibraryView | None = None
        self.watchlist: WatchlistView | None = None
        self._worker: Thread | None = None
        self._settings_worker: Thread | None = None
        self._library_worker: Thread | None = None
        self._cancellation: CancellationToken | None = None
        self._worker_return_target = "launcher"
        self._return_after_worker = False
        self._auto_return_after_worker = False
        self._worker_label = "Workflow"
        self._worker_error: Exception | None = None

    def run(self) -> None:
        """Create the viewport and run callbacks on the main render thread."""
        dpg.create_context()
        try:
            dpg.configure_app(manual_callback_management=True)
            apply_global_theme()
            self.launcher.build()
            dpg.create_viewport(title="muzik", width=1280, height=800)
            dpg.setup_dearpygui()
            dpg.show_viewport()
            dpg.set_primary_window(LAUNCHER_WINDOW, True)
            while dpg.is_dearpygui_running():
                dpg.run_callbacks(dpg.get_callback_queue())
                self.bridge.drain()
                self._poll_worker()
                if self.watchlist is not None and dpg.does_item_exist(WATCHLIST_WINDOW):
                    self.watchlist.set_available_width(
                        max(300, dpg.get_viewport_client_width() - 290)
                    )
                dpg.render_dearpygui_frame()
        finally:
            self._cancel_worker()
            self.bridge.shutdown()
            if self._worker is not None:
                self._worker.join(timeout=5)
            if self._settings_worker is not None:
                self._settings_worker.join(timeout=5)
            if self._library_worker is not None:
                self._library_worker.join(timeout=5)
            dpg.destroy_context()

    def open_pipeline(self, config: WorkflowLaunchConfig) -> None:
        """Build the pipeline view and start its workflow worker."""
        if self._worker is not None and self._worker.is_alive():
            return
        self.launcher.hide()
        self._worker_return_target = "launcher"
        self._return_after_worker = False
        self._auto_return_after_worker = False
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
        """Cancel an active run and return after its worker has stopped."""
        if self._worker is not None and self._worker.is_alive():
            self._return_after_worker = True
            self._cancel_worker()
            modals.close_all_modals()
            if self.pipeline is not None:
                self.pipeline.set_status("Cancelling...")
                self.pipeline.disable_back()
            return
        self._show_worker_return_target()

    def open_watchlist(
        self,
        sender: Any = None,
        app_data: Any = None,
        user_data: Any = None,
    ) -> None:
        """Open the saved playlist watchlist without network work."""
        if self._worker is not None and self._worker.is_alive():
            return
        self.launcher.hide()
        if self.watchlist is None:
            self.watchlist = WatchlistView(
                on_add=self.add_watchlist_playlist,
                on_remove=self.remove_watchlist_playlist,
                on_refresh=self.refresh_watchlist,
                on_action=self.request_item_action,
                on_back=self.close_watchlist,
                on_quit=self.quit,
            )
            self.watchlist.build()
        else:
            self.watchlist.show()
        self._reload_watchlist()

    def close_watchlist(
        self,
        sender: Any = None,
        app_data: Any = None,
        user_data: Any = None,
    ) -> None:
        """Close the watchlist and release its image textures."""
        if self.watchlist is not None:
            self.watchlist.destroy()
        self.watchlist = None
        self.launcher.show()

    def add_watchlist_playlist(self, url: str) -> None:
        """Add one normalized YouTube playlist and reload the rail."""
        try:
            self.watchlist_repository.add(url)
        except WatchlistError as exc:
            if self.watchlist is not None:
                self.watchlist.show_error(str(exc))
            return
        if self.watchlist is not None:
            self.watchlist.clear_error()
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
        if self.watchlist is not None:
            self.watchlist.hide()
        self._worker_return_target = "watchlist"
        self._return_after_worker = False
        self._auto_return_after_worker = True
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
        """Open the settings window and start the service checks."""
        if dpg.does_item_exist(SETTINGS_WINDOW):
            return
        if self.settings is None:
            self.settings = SettingsView(self.recheck_services, self.close_settings)
        self.settings.build()
        self._start_service_checks()

    def recheck_services(
        self,
        sender: Any = None,
        app_data: Any = None,
        user_data: Any = None,
    ) -> None:
        """Re-run the service checks for the open settings window."""
        self._start_service_checks()

    def close_settings(
        self,
        sender: Any = None,
        app_data: Any = None,
        user_data: Any = None,
    ) -> None:
        """Close the settings window."""
        if self.settings is not None:
            self.settings.destroy()

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
        """Open the library window and scan the output folder."""
        if dpg.does_item_exist(LIBRARY_WINDOW):
            return
        if self.library is None:
            self.library = LibraryView(
                DEFAULT_DOWNLOAD_DIR,
                self.refresh_library,
                self.close_library,
            )
        self.launcher.hide()
        self.library.build()
        dpg.set_primary_window(LIBRARY_WINDOW, True)
        self._start_library_scan()

    def refresh_library(
        self,
        sender: Any = None,
        app_data: Any = None,
        user_data: Any = None,
    ) -> None:
        """Re-scan the output folder for the open library window."""
        self._start_library_scan()

    def close_library(
        self,
        sender: Any = None,
        app_data: Any = None,
        user_data: Any = None,
    ) -> None:
        """Close the library page and return to the launcher."""
        if self.library is not None:
            self.library.destroy()
        self.launcher.show()

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
                cancellation=cancellation,
                on_state_change=lambda: self.watchlist_repository.save(watchlist),
            )
            if (
                item.video_id
                and item_summary_state(item) == "Processed"
                and item.video_id not in playlist.processed_video_ids
            ):
                playlist.processed_video_ids.append(item.video_id)
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
        if self._return_after_worker or self._auto_return_after_worker:
            self._show_worker_return_target()
            return
        if self.pipeline is not None:
            if self._worker_error is None:
                self.pipeline.set_status("Complete")
                self.pipeline.log(f"{self._worker_label} complete.")
            else:
                self.pipeline.set_status("Failed")
                self.pipeline.log(f"{self._worker_label} failed: {self._worker_error}")
        self._worker = None

    def _show_worker_return_target(self) -> None:
        if self._worker_return_target == "watchlist":
            self._show_watchlist()
        else:
            self._show_launcher()

    def _show_watchlist(self) -> None:
        error = self._worker_error
        if self.pipeline is not None:
            self.pipeline.destroy()
        self.pipeline = None
        self._worker = None
        self._cancellation = None
        self._return_after_worker = False
        self._auto_return_after_worker = False
        if self.watchlist is None:
            self.open_watchlist()
        else:
            self.watchlist.show()
            self._reload_watchlist()
        if error is not None and self.watchlist is not None:
            self.watchlist.show_error(str(error))
        self._worker_error = None

    def _reload_watchlist(self) -> None:
        view = self.watchlist
        if view is None:
            return
        try:
            config = self.launcher.read_config()
            request = WorkflowRequest("", config.output, config.splits)
            options = _workflow_options(config)
            watchlist = self.watchlist_repository.load()
            reconcile_watchlist(watchlist, request=request, options=options)
            self.watchlist_repository.save(watchlist)
        except (WatchlistError, TypeError, ValueError) as exc:
            view.show_error(str(exc))
            return
        view.load(watchlist, request)
        for playlist in watchlist.playlists:
            for item in playlist.items:
                if not item.video_id:
                    continue
                path = cached_thumbnail_path(item.video_id)
                if path is None:
                    continue

                def load_thumbnail(
                    selected_view: WatchlistView = view,
                    video_id: str = item.video_id,
                    thumbnail_path=path,
                ) -> None:
                    if self.watchlist is selected_view:
                        selected_view.load_cached_thumbnail(video_id, thumbnail_path)

                self.bridge.submit(load_thumbnail)

    def _show_launcher(self) -> None:
        if self.pipeline is not None:
            self.pipeline.destroy()
        self.pipeline = None
        self._worker = None
        self._cancellation = None
        self._return_after_worker = False
        self._auto_return_after_worker = False
        self._worker_return_target = "launcher"
        self._worker_error = None
        self.launcher.show()


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

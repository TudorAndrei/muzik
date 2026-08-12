"""DearPyGui application entry point and workflow runner."""

from __future__ import annotations

from collections.abc import Callable
import inspect
from threading import Thread
from typing import Any

import dearpygui.dearpygui as dpg

from muzik.core.workflow.cancellation import CancellationToken, WorkflowCancelled
from muzik.core.workflow.decisions import WorkflowDecisions
from muzik.core.workflow.events import ErrorEvent, WorkflowEventEmitter
from muzik.core.workflow.launch import WorkflowLaunchConfig
from muzik.core.workflow.operations import build_workflow_operations
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
from muzik.gui.bridge import GuiBridge
from muzik.gui.launcher import LAUNCHER_WINDOW, LauncherView
from muzik.gui.pipeline import PipelineView


WorkflowOperationsFactory = Callable[..., WorkflowRunOperations]


class MuzikGuiApp:
    """Own the desktop render loop and one background workflow at a time."""

    def __init__(
        self,
        *,
        operations_factory: WorkflowOperationsFactory | None = None,
    ) -> None:
        self.operations_factory = operations_factory or _default_operations
        self.bridge = GuiBridge(on_error=self._handle_bridge_error)
        self.launcher = LauncherView(self.open_pipeline, self.quit)
        self.pipeline: PipelineView | None = None
        self._worker: Thread | None = None
        self._cancellation: CancellationToken | None = None
        self._return_to_launcher = False
        self._worker_error: Exception | None = None

    def run(self) -> None:
        """Create the viewport and run callbacks on the main render thread."""
        dpg.create_context()
        try:
            dpg.configure_app(manual_callback_management=True)
            self.launcher.build()
            dpg.create_viewport(title="muzik", width=1280, height=800)
            dpg.setup_dearpygui()
            dpg.show_viewport()
            dpg.set_primary_window(LAUNCHER_WINDOW, True)
            while dpg.is_dearpygui_running():
                dpg.run_callbacks(dpg.get_callback_queue())
                self.bridge.drain()
                self._poll_worker()
                dpg.render_dearpygui_frame()
        finally:
            self._cancel_worker()
            self.bridge.shutdown()
            if self._worker is not None:
                self._worker.join(timeout=5)
            dpg.destroy_context()

    def open_pipeline(self, config: WorkflowLaunchConfig) -> None:
        """Build the pipeline view and start its workflow worker."""
        if self._worker is not None and self._worker.is_alive():
            return
        self.launcher.hide()
        self._return_to_launcher = False
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
            self._return_to_launcher = True
            self._cancel_worker()
            modals.close_all_modals()
            if self.pipeline is not None:
                self.pipeline.set_status("Cancelling...")
                self.pipeline.disable_back()
            return
        self._show_launcher()

    def quit(
        self,
        sender: Any = None,
        app_data: Any = None,
        user_data: Any = None,
    ) -> None:
        """Cancel active work and stop the viewport."""
        self._cancel_worker()
        modals.close_all_modals()
        dpg.stop_dearpygui()

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
        if self._return_to_launcher:
            self._show_launcher()
            return
        if self.pipeline is not None:
            if self._worker_error is None:
                self.pipeline.set_status("Complete")
                self.pipeline.log("Workflow complete.")
            else:
                self.pipeline.set_status("Failed")
                self.pipeline.log(f"Workflow failed: {self._worker_error}")
        self._worker = None

    def _show_launcher(self) -> None:
        if self.pipeline is not None:
            self.pipeline.destroy()
        self.pipeline = None
        self._worker = None
        self._cancellation = None
        self._return_to_launcher = False
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

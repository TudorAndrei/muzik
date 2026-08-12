from threading import Event
import time

import dearpygui.dearpygui as dpg

from muzik.core.workflow.cancellation import WorkflowCancelled
from muzik.core.workflow.launch import WorkflowLaunchConfig
from muzik.core.workflow.service import WorkflowRunOperations
from muzik.gui.app import MuzikGuiApp


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

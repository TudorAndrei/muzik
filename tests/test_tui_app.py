import asyncio
from pathlib import Path
from threading import Event
from typing import cast

from textual.widgets import Input, Switch

from muzik.core.workflow.service import WorkflowRunOperations
from muzik.tui.app import MuzikTuiApp, PipelineScreen
from muzik.tui.screens import WorkflowLaunchConfig, WorkflowLauncherScreen


def test_tui_starts_on_workflow_launcher() -> None:
    async def run() -> None:
        app = MuzikTuiApp()
        async with app.run_test() as pilot:
            await pilot.pause()
            assert isinstance(app.screen, WorkflowLauncherScreen)

    asyncio.run(run())


def test_tui_command_palette_opens_and_has_workflow_commands() -> None:
    async def run() -> None:
        app = MuzikTuiApp()
        async with app.run_test() as pilot:
            await pilot.pause()
            screen = cast(WorkflowLauncherScreen, app.screen)
            command_titles = {
                command.title for command in app.get_system_commands(screen)
            }

            await pilot.press("ctrl+p")
            await pilot.pause()

            assert app.screen.id == "--command-palette"
            assert "Run workflow" in command_titles
            assert "Quit" in command_titles

    asyncio.run(run())


def test_workflow_launcher_reads_config() -> None:
    async def run() -> None:
        app = MuzikTuiApp()
        async with app.run_test() as pilot:
            await pilot.pause()
            screen = cast(WorkflowLauncherScreen, app.screen)
            screen.query_one("#raw", Input).value = "https://example.test/video"
            screen.query_one("#output", Input).value = "~/Downloads/muzik"
            screen.query_one("#splits", Input).value = "~/Music/splits"
            screen.query_one("#jobs", Input).value = "2"
            screen.query_one("#dry-run", Switch).value = True
            screen.query_one("#interactive", Switch).value = False

            config = screen.read_config()

            assert config.raw == "https://example.test/video"
            assert config.output == Path("~/Downloads/muzik").expanduser()
            assert config.splits == Path("~/Music/splits").expanduser()
            assert config.jobs == 2
            assert config.dry_run is True
            assert config.interactive is False

    asyncio.run(run())


def test_pipeline_screen_runs_workflow_in_worker() -> None:
    processed = []
    done = Event()

    def operations_factory(config, decisions, events):
        def process_audio(audio_inputs, pre_split_dirs):
            processed.append((audio_inputs, pre_split_dirs))
            done.set()

        return WorkflowRunOperations(
            download_audio=lambda url, output, archive_file: True,
            process_audio=process_audio,
            acquire_soulseek=lambda raw: [],
            prepopulate_archive=lambda archive_file: None,
            get_playlist_video_ids=lambda raw: [],
        )

    async def run() -> None:
        config = WorkflowLaunchConfig(raw="local-input", dry_run=True)
        app = MuzikTuiApp(operations_factory=operations_factory)
        async with app.run_test() as pilot:
            await app.push_screen(
                PipelineScreen(config, operations_factory=operations_factory)
            )
            await asyncio.to_thread(done.wait, 2)
            await pilot.pause()

            assert done.is_set()
            assert processed == [([], [])]

    asyncio.run(run())


def test_pipeline_back_returns_to_launcher() -> None:
    done = Event()

    def operations_factory(config, decisions, events):
        def process_audio(audio_inputs, pre_split_dirs):
            done.set()

        return WorkflowRunOperations(
            download_audio=lambda url, output, archive_file: True,
            process_audio=process_audio,
            acquire_soulseek=lambda raw: [],
            prepopulate_archive=lambda archive_file: None,
            get_playlist_video_ids=lambda raw: [],
        )

    async def run() -> None:
        config = WorkflowLaunchConfig(raw="local-input", dry_run=True)
        app = MuzikTuiApp(operations_factory=operations_factory)
        async with app.run_test() as pilot:
            await app.push_screen(
                PipelineScreen(config, operations_factory=operations_factory)
            )
            await asyncio.to_thread(done.wait, 2)
            await app.open_launcher()
            await pilot.pause()

            assert isinstance(app.screen, WorkflowLauncherScreen)

    asyncio.run(run())

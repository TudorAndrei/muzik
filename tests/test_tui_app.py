import asyncio
from pathlib import Path
from threading import Event
from typing import cast

from textual.widgets import Input, Switch

from muzik.core import cache as cache_mod
from muzik.core.beets.decisions import BeetsDuplicateDecision, BeetsMatchDecision
from muzik.core.beets.views import BeetsDuplicateView, BeetsMatchView, BeetsTaskView
from muzik.core.workflow.service import WorkflowRunOperations
from muzik.core.workflow.service import AudioSource
from muzik.tui.app import MuzikTuiApp, PipelineScreen
from muzik.tui.screens import (
    BeetsMatchScreen,
    DuplicateResolutionScreen,
    WorkflowLaunchConfig,
    WorkflowLauncherScreen,
)


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


def test_pipeline_back_waits_for_workflow_teardown() -> None:
    started = Event()
    release = Event()

    def operations_factory(config, decisions, events):
        def process_audio(audio_inputs, pre_split_dirs):
            started.set()
            release.wait(2)

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
            await asyncio.to_thread(started.wait, 2)
            await pilot.click("#back")
            await pilot.pause()

            assert isinstance(app.screen, PipelineScreen)

            release.set()
            for _ in range(10):
                await pilot.pause()
                if isinstance(app.screen, WorkflowLauncherScreen):
                    break
            assert isinstance(app.screen, WorkflowLauncherScreen)

    asyncio.run(run())


def test_pipeline_routes_spotify_export_file_to_soulseek(
    tmp_path: Path,
    monkeypatch,
) -> None:
    """The actual TUI worker handles a Spotify file input without yt-dlp."""
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    export = Path("tests/fixtures/spotify/playlist_v1.json").resolve()
    audio = tmp_path / "downloads" / "fixture.flac"
    audio.parent.mkdir()
    audio.write_bytes(b"audio")
    acquired: list[str] = []
    processed: list[list[Path]] = []
    done = Event()

    def operations_factory(config, decisions, events):
        def process_audio(audio_inputs, pre_split_dirs):
            processed.append(audio_inputs)
            done.set()

        return WorkflowRunOperations(
            download_audio=lambda *args: (_ for _ in ()).throw(
                AssertionError("Spotify media must never reach yt-dlp")
            ),
            process_audio=process_audio,
            acquire_soulseek=lambda query: acquired.append(query) or [audio],
            prepopulate_archive=lambda archive_file: None,
            get_playlist_video_ids=lambda raw: [],
        )

    async def run() -> None:
        config = WorkflowLaunchConfig(
            raw=str(export),
            output=tmp_path / "downloads",
            splits=tmp_path / "splits",
            no_organize=True,
            audio_source=AudioSource.SOULSEEK,
        )
        app = MuzikTuiApp(operations_factory=operations_factory)
        async with app.run_test() as pilot:
            await app.push_screen(
                PipelineScreen(config, operations_factory=operations_factory)
            )
            await asyncio.to_thread(done.wait, 2)
            await pilot.pause()

            assert acquired == ["Fixture artist - Fixture song - Fixture album"]
            assert processed == [[audio]]

    asyncio.run(run())


def test_beets_match_modal_returns_selected_as_is_and_skip_actions() -> None:
    task = BeetsTaskView(
        task_id="task-1",
        matches=[BeetsMatchView(candidate_id="candidate-1", artist="Artist")],
    )

    async def run() -> None:
        app = MuzikTuiApp()
        async with app.run_test() as pilot:
            for button_id, expected in (
                ("#select-match", "candidate-1"),
                ("#as-is-match", BeetsMatchDecision.AS_IS),
                ("#skip-match", None),
            ):
                result = []
                await app.push_screen(BeetsMatchScreen(task), callback=result.append)
                assert isinstance(app.screen, BeetsMatchScreen)
                await pilot.click(button_id)
                await pilot.pause()
                assert result == [expected]

    asyncio.run(run())


def test_duplicate_modal_returns_each_duplicate_action() -> None:
    duplicates = [BeetsDuplicateView(artist="Artist", title="Song")]

    async def run() -> None:
        app = MuzikTuiApp()
        async with app.run_test() as pilot:
            for button_id, expected in (
                ("#duplicate-skip", BeetsDuplicateDecision.SKIP),
                ("#duplicate-keep", BeetsDuplicateDecision.KEEP_ALL),
                ("#duplicate-remove", BeetsDuplicateDecision.REMOVE_OLD),
                ("#duplicate-merge", BeetsDuplicateDecision.MERGE),
            ):
                result = []
                await app.push_screen(
                    DuplicateResolutionScreen(duplicates), callback=result.append
                )
                assert isinstance(app.screen, DuplicateResolutionScreen)
                await pilot.click(button_id)
                await pilot.pause()
                assert result == [expected]

    asyncio.run(run())


def test_pipeline_cancellation_dismisses_a_pending_beets_modal() -> None:
    started = Event()
    stopped = Event()
    task = BeetsTaskView(
        task_id="task-1",
        matches=[BeetsMatchView(candidate_id="candidate-1", artist="Artist")],
    )

    def operations_factory(config, decisions, events, beets_decisions, beets_events):
        def process_audio(audio_inputs, pre_split_dirs):
            started.set()
            try:
                beets_decisions.choose_beets_album_match(task)
            finally:
                stopped.set()

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
        pipeline = PipelineScreen(config, operations_factory=operations_factory)
        async with app.run_test() as pilot:
            await app.push_screen(pipeline)
            await asyncio.to_thread(started.wait, 2)
            for _ in range(10):
                await pilot.pause()
                if isinstance(app.screen, BeetsMatchScreen):
                    break
            assert isinstance(app.screen, BeetsMatchScreen)

            assert pipeline._request_return_to_launcher() is True
            await asyncio.to_thread(stopped.wait, 2)
            for _ in range(10):
                await pilot.pause()
                if isinstance(app.screen, WorkflowLauncherScreen):
                    break
            assert isinstance(app.screen, WorkflowLauncherScreen)

    asyncio.run(run())

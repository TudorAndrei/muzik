from pathlib import Path

import dearpygui.dearpygui as dpg

from muzik.core.workflow.service import AudioFallback, AudioSource, MetadataSource
from muzik.gui.launcher import FIELD_TAGS, ERROR_TEXT, LauncherView, config_from_values


def test_launcher_maps_every_field_and_coerces_enums() -> None:
    config = config_from_values(
        {
            "raw": "  https://example.test/album  ",
            "output": "~/Downloads/muzik",
            "splits": "~/Music/splits",
            "review": True,
            "no_split": True,
            "no_organize": True,
            "import_": True,
            "tag_only": True,
            "dry_run": True,
            "jobs": "3",
            "config": "~/.config/beets/config.yaml",
            "keep_source": True,
            "force": True,
            "metadata_source": "musicbrainz",
            "audio_source": "soulseek",
            "prefer": "flac",
            "fallback": "none",
            "interactive": False,
        }
    )

    assert config.raw == "https://example.test/album"
    assert config.output == Path("~/Downloads/muzik").expanduser()
    assert config.splits == Path("~/Music/splits").expanduser()
    assert config.review is True
    assert config.no_split is True
    assert config.no_organize is True
    assert config.import_ is True
    assert config.tag_only is True
    assert config.dry_run is True
    assert config.jobs == 3
    assert config.config == Path("~/.config/beets/config.yaml").expanduser()
    assert config.keep_source is True
    assert config.force is True
    assert config.metadata_source is MetadataSource.MUSICBRAINZ
    assert config.audio_source is AudioSource.SOULSEEK
    assert config.prefer == "flac"
    assert config.fallback is AudioFallback.NONE
    assert config.interactive is False


def test_launcher_empty_jobs_and_config_use_defaults() -> None:
    config = config_from_values(
        {
            "raw": "local.flac",
            "output": "/tmp/downloads",
            "splits": "/tmp/splits",
            "jobs": "",
            "config": "",
        }
    )

    assert config.jobs == 0
    assert config.config is None


def test_read_config_reads_dearpygui_fields() -> None:
    dpg.create_context()
    launcher = LauncherView(lambda config: None, lambda: None)
    try:
        launcher.build()
        dpg.set_value(FIELD_TAGS["raw"], "local.flac")
        dpg.set_value(FIELD_TAGS["jobs"], 4)
        dpg.set_value(FIELD_TAGS["audio_source"], "auto")
        dpg.set_value(FIELD_TAGS["metadata_source"], "none")

        config = launcher.read_config()

        assert config.raw == "local.flac"
        assert config.jobs == 4
        assert config.audio_source is AudioSource.AUTO
        assert config.metadata_source is MetadataSource.NONE
    finally:
        dpg.destroy_context()


def test_run_rejects_empty_input() -> None:
    runs = []
    dpg.create_context()
    launcher = LauncherView(runs.append, lambda: None)
    try:
        launcher.build()

        launcher._run()

        assert runs == []
        assert dpg.get_value(ERROR_TEXT) == "Enter a URL or local path."
    finally:
        dpg.destroy_context()

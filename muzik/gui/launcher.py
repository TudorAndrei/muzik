"""Workflow launcher form for the desktop interface."""

from __future__ import annotations

from collections.abc import Callable, Mapping
from pathlib import Path
from typing import Any

import dearpygui.dearpygui as dpg

from muzik.config import DEFAULT_DOWNLOAD_DIR, DEFAULT_SPLITS_DIR
from muzik.core.workflow.launch import WorkflowLaunchConfig
from muzik.core.workflow.service import AudioFallback, AudioSource, MetadataSource
from muzik.gui.theme import ACCENT, bind_primary_button


LAUNCHER_WINDOW = "launcher-window"
ERROR_TEXT = "launcher-error"

FIELD_TAGS = {
    "raw": "launcher-raw",
    "output": "launcher-output",
    "splits": "launcher-splits",
    "config": "launcher-config",
    "audio_source": "launcher-audio-source",
    "metadata_source": "launcher-metadata-source",
    "prefer": "launcher-prefer",
    "fallback": "launcher-fallback",
    "jobs": "launcher-jobs",
    "review": "launcher-review",
    "no_split": "launcher-no-split",
    "no_organize": "launcher-no-organize",
    "import_": "launcher-import",
    "tag_only": "launcher-tag-only",
    "dry_run": "launcher-dry-run",
    "keep_source": "launcher-keep-source",
    "force": "launcher-force",
    "interactive": "launcher-interactive",
}


def config_from_values(values: Mapping[str, Any]) -> WorkflowLaunchConfig:
    """Convert launcher values to a core launch configuration."""
    jobs_raw = str(values.get("jobs", "")).strip()
    config_raw = str(values.get("config", "")).strip()
    return WorkflowLaunchConfig(
        raw=str(values.get("raw", "")).strip(),
        output=Path(str(values.get("output", DEFAULT_DOWNLOAD_DIR))).expanduser(),
        splits=Path(str(values.get("splits", DEFAULT_SPLITS_DIR))).expanduser(),
        review=bool(values.get("review", False)),
        no_split=bool(values.get("no_split", False)),
        no_organize=bool(values.get("no_organize", False)),
        import_=bool(values.get("import_", False)),
        tag_only=bool(values.get("tag_only", False)),
        dry_run=bool(values.get("dry_run", False)),
        jobs=int(jobs_raw or "0"),
        config=Path(config_raw).expanduser() if config_raw else None,
        keep_source=bool(values.get("keep_source", False)),
        force=bool(values.get("force", False)),
        metadata_source=MetadataSource(str(values.get("metadata_source", "auto"))),
        audio_source=AudioSource(str(values.get("audio_source", "youtube"))),
        prefer=str(values.get("prefer", "lossless")),
        fallback=AudioFallback(str(values.get("fallback", "youtube"))),
        interactive=bool(values.get("interactive", True)),
    )


class LauncherView:
    """Build and read the desktop workflow launcher."""

    def __init__(
        self,
        on_run: Callable[[WorkflowLaunchConfig], None],
        on_quit: Callable[[], None],
        on_settings: Callable[..., None],
        on_library: Callable[..., None],
    ) -> None:
        self._on_run = on_run
        self._on_quit = on_quit
        self._on_settings = on_settings
        self._on_library = on_library

    def build(self) -> None:
        with dpg.window(tag=LAUNCHER_WINDOW, label="muzik workflow"):
            dpg.add_text("Workflow", color=ACCENT)
            dpg.add_separator()
            self._path_row("URL or path", "raw", "raw-file-dialog", False, "")
            self._path_row(
                "Downloads",
                "output",
                "output-file-dialog",
                True,
                str(DEFAULT_DOWNLOAD_DIR),
            )
            self._path_row(
                "Splits",
                "splits",
                "splits-file-dialog",
                True,
                str(DEFAULT_SPLITS_DIR),
            )
            self._path_row("Beets config", "config", "config-file-dialog", False, "")

            dpg.add_separator()
            with dpg.group(horizontal=True):
                dpg.add_combo(
                    ["youtube", "soulseek", "auto"],
                    default_value="youtube",
                    label="Audio source",
                    tag=FIELD_TAGS["audio_source"],
                    width=180,
                )
                dpg.add_combo(
                    ["auto", "youtube", "musicbrainz", "none"],
                    default_value="auto",
                    label="Metadata",
                    tag=FIELD_TAGS["metadata_source"],
                    width=180,
                )
            with dpg.group(horizontal=True):
                dpg.add_combo(
                    ["lossless", "best", "mp3", "flac"],
                    default_value="lossless",
                    label="Prefer",
                    tag=FIELD_TAGS["prefer"],
                    width=180,
                )
                dpg.add_combo(
                    ["youtube", "none"],
                    default_value="youtube",
                    label="Fallback",
                    tag=FIELD_TAGS["fallback"],
                    width=180,
                )
                dpg.add_input_int(label="Jobs", tag=FIELD_TAGS["jobs"], width=90)

            dpg.add_separator()
            switches = [
                ("Review chapters", "review", False),
                ("No split", "no_split", False),
                ("No organize", "no_organize", False),
                ("Import", "import_", False),
                ("Tag only", "tag_only", False),
                ("Dry run", "dry_run", False),
                ("Keep source", "keep_source", False),
                ("Force", "force", False),
                ("Interactive", "interactive", True),
            ]
            for start in range(0, len(switches), 3):
                with dpg.group(horizontal=True):
                    for label, name, default in switches[start : start + 3]:
                        dpg.add_checkbox(
                            label=label,
                            default_value=default,
                            tag=FIELD_TAGS[name],
                        )

            dpg.add_text("", tag=ERROR_TEXT, color=(255, 100, 100))
            with dpg.group(horizontal=True):
                run_button = dpg.add_button(label="Run", callback=self._run, width=100)
                dpg.add_button(label="Library", callback=self._on_library, width=100)
                dpg.add_button(label="Settings", callback=self._on_settings, width=100)
                dpg.add_button(label="Quit", callback=self._quit, width=100)
            bind_primary_button(run_button)

    def read_config(self) -> WorkflowLaunchConfig:
        values = {name: dpg.get_value(tag) for name, tag in FIELD_TAGS.items()}
        return config_from_values(values)

    def show(self) -> None:
        dpg.show_item(LAUNCHER_WINDOW)
        dpg.set_primary_window(LAUNCHER_WINDOW, True)

    def hide(self) -> None:
        dpg.hide_item(LAUNCHER_WINDOW)

    def _run(
        self,
        sender: Any = None,
        app_data: Any = None,
        user_data: Any = None,
    ) -> None:
        try:
            config = self.read_config()
        except (TypeError, ValueError) as exc:
            dpg.set_value(ERROR_TEXT, f"Invalid launcher value: {exc}")
            return
        if not config.raw:
            dpg.set_value(ERROR_TEXT, "Enter a URL or local path.")
            return
        dpg.set_value(ERROR_TEXT, "")
        self._on_run(config)

    def _quit(
        self,
        sender: Any = None,
        app_data: Any = None,
        user_data: Any = None,
    ) -> None:
        self._on_quit()

    @staticmethod
    def _path_row(
        label: str,
        field: str,
        dialog_tag: str,
        directory: bool,
        default: str,
    ) -> None:
        def selected(
            sender: Any,
            app_data: Mapping[str, Any],
            user_data: Any = None,
        ) -> None:
            value = app_data.get("file_path_name") or app_data.get("current_path")
            if value:
                dpg.set_value(FIELD_TAGS[field], str(value))

        with dpg.group(horizontal=True):
            # Label on the left as fixed-width text; DearPyGui's own labels sit to
            # the right of a field and would push the Browse button off-screen.
            dpg.add_text(f"{label:<13}")
            dpg.add_input_text(
                default_value=default,
                tag=FIELD_TAGS[field],
                width=-110,
            )
            dpg.add_button(
                label="Browse...",
                callback=lambda s=None, a=None, u=None: dpg.show_item(dialog_tag),
                width=90,
            )
        with dpg.file_dialog(
            tag=dialog_tag,
            show=False,
            directory_selector=directory,
            callback=selected,
            width=760,
            height=440,
        ):
            if not directory:
                dpg.add_file_extension(".*")

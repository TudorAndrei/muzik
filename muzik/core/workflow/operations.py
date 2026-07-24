"""Concrete core operations for CLI and TUI workflow adapters."""

from __future__ import annotations

import os
from pathlib import Path
from typing import cast

from muzik.core.audio import extract_metadata, get_duration
from muzik.core.beets.decisions import BeetsDecisions, NonInteractiveBeetsDecisions
from muzik.core.beets.events import BeetsEventEmitter, NullBeetsEventEmitter
from muzik.core.beets.importer import ImportOptions
from muzik.core.beets.service import organize_paths, tag_only_with_beet
from muzik.core.chapters import Chapter, find_chapters, serialize_chapters
from muzik.core.description_chapters import (
    description_has_timestamps,
    extract_chapters_from_description,
    get_description_from_info_json,
)
from muzik.core.musicbrainz import MIN_ALBUM_DURATION, lookup_chapters_verbose
from muzik.core.sources.base import Candidate
from muzik.core.sources.soulseek import SoulseekSource
from muzik.core.sources.youtube import (
    YouTubeSource,
    get_playlist_video_ids,
    prepopulate_archive,
)
from muzik.core.splitter import SplitError, split_audio
from muzik.core.workflow.cancellation import CancellationToken
from muzik.core.workflow.decisions import WorkflowDecisions
from muzik.core.workflow.events import (
    ChapterReviewRequestedEvent,
    MessageEvent,
    NullWorkflowEventEmitter,
    WorkflowEventEmitter,
)
from muzik.core.workflow.decisions import ChapterDecision
from muzik.core.workflow.service import (
    AudioFallback,
    SplitTask,
    WorkflowOptions,
    WorkflowRunOperations,
    WorkflowServiceError,
    MetadataWorkflowSource,
    SoulseekWorkflowSource,
    acquire_from_soulseek,
    find_audio_inputs,
    process_audio_plan,
    validated_audio_files,
)


def build_workflow_operations(
    *,
    splits: Path,
    options: WorkflowOptions,
    decisions: WorkflowDecisions,
    events: WorkflowEventEmitter | None = None,
    beets_decisions: BeetsDecisions | None = None,
    beets_events: BeetsEventEmitter | None = None,
) -> WorkflowRunOperations:
    """Build concrete operations without binding to a terminal or Textual."""
    events = events or NullWorkflowEventEmitter()
    beets_decisions = beets_decisions or NonInteractiveBeetsDecisions()
    beets_events = beets_events or NullBeetsEventEmitter()

    def download_audio(
        url: str,
        output: Path,
        archive_file: Path | None,
        *,
        cancellation: CancellationToken | None = None,
    ) -> bool:
        source = YouTubeSource()
        candidate = Candidate(source="youtube", source_id=url, title=url, path=url)
        try:
            source.download(
                candidate,
                output,
                archive_file=archive_file,
                cancellation=cancellation,
            )
        except RuntimeError:
            return False
        return True

    def process_audio(
        audio_inputs: list[Path],
        pre_split_dirs: list[Path],
        *,
        cancellation: CancellationToken | None = None,
    ) -> None:
        audio_files = find_audio_inputs(audio_inputs)
        audio_files, warnings = validated_audio_files(
            audio_files,
            dry_run=options.dry_run,
            no_organize=options.no_organize,
            duration_probe=get_duration,
        )
        for warning in warnings:
            events.emit(MessageEvent(warning, severity="warning"))
        if not audio_files and not pre_split_dirs and not options.dry_run:
            raise WorkflowServiceError(
                "No audio files found in output directory.", exit_code=0
            )

        def split_operation(task: SplitTask) -> bool:
            chapters = find_chapters(task.source)
            try:
                split_audio(
                    task.source,
                    chapters,
                    output=task.output,
                    jobs=options.jobs,
                    keep_source=options.keep_source,
                    force=options.force,
                    cancellation=cancellation,
                )
            except SplitError:
                return False
            return True

        def organize_operation(target: Path) -> bool:
            try:
                organize_paths(
                    ImportOptions(
                        paths=[target],
                        config_path=options.config
                        if options.config and options.config.exists()
                        else None,
                        move=True,
                        dry_run=options.dry_run,
                        incremental=True,
                    ),
                    tag_only=options.tag_only,
                    decisions=beets_decisions,
                    events=beets_events,
                    tag_only_runner=tag_only_with_beet if options.tag_only else None,
                )
            except Exception:
                return False
            return True

        process_audio_plan(
            audio_files=audio_files,
            pre_split_dirs=pre_split_dirs,
            splits=splits,
            options=options,
            chapter_resolver=lambda path: _chapters_for(
                path, options, decisions, events
            ),
            split_operation=split_operation,
            organize_operation=organize_operation,
            events=events,
            cancellation=cancellation,
        )

    return WorkflowRunOperations(
        download_audio=download_audio,
        process_audio=process_audio,
        acquire_soulseek=lambda raw, *, cancellation=None: acquire_from_soulseek(
            raw,
            prefer=options.prefer,
            fallback=AudioFallback(options.fallback).value,
            decisions=decisions,
            events=events,
            source_factory=lambda: cast(SoulseekWorkflowSource, SoulseekSource()),
            youtube_source_factory=lambda: cast(
                MetadataWorkflowSource, YouTubeSource()
            ),
            cancellation=cancellation,
        ),
        prepopulate_archive=lambda archive: _prepopulate_archive(archive),
        get_playlist_video_ids=get_playlist_video_ids,
        soulseek_ready=_soulseek_ready,
    )


def _chapters_for(
    path: Path,
    options: WorkflowOptions,
    decisions: WorkflowDecisions,
    events: WorkflowEventEmitter,
) -> list[Chapter] | None:
    if options.no_split:
        return None
    chapters = find_chapters(path)
    if chapters:
        return chapters
    if options.metadata_source == "none":
        return None
    duration = get_duration(path)
    if not duration or duration < MIN_ALBUM_DURATION:
        return None
    if options.metadata_source == "youtube":
        return _description_chapters(path, decisions, events)
    metadata = extract_metadata(path)
    chapters, title, diagnostics = lookup_chapters_verbose(
        metadata.get("artist", ""),
        metadata.get("album", ""),
        metadata.get("year", ""),
    )
    if not chapters:
        events.emit(
            MessageEvent(
                f"MusicBrainz: no match found. {diagnostics}", severity="debug"
            )
        )
        if options.metadata_source == "musicbrainz":
            return None
        return _description_chapters(path, decisions, events)
    events.emit(
        ChapterReviewRequestedEvent(
            source=path,
            chapters=chapters,
            title=f"MusicBrainz — {title}",
        )
    )
    choice = decisions.confirm_chapters(path, chapters)
    if choice == ChapterDecision.EDIT:
        chapters = decisions.edit_chapters(chapters) or []
    if not chapters:
        return None
    path.with_suffix(".chapters.txt").write_text(
        serialize_chapters(chapters), encoding="utf-8"
    )
    return chapters


def _description_chapters(
    path: Path,
    decisions: WorkflowDecisions,
    events: WorkflowEventEmitter,
) -> list[Chapter] | None:
    info_path = path.with_suffix("").with_suffix(".info.json")
    if not info_path.exists() or not os.environ.get("OPENROUTER_API_KEY"):
        return None
    description = get_description_from_info_json(info_path)
    if not description or not description_has_timestamps(description):
        return None
    chapters, error = extract_chapters_from_description(description)
    if error or not chapters:
        return None
    events.emit(
        ChapterReviewRequestedEvent(
            source=path, chapters=chapters, title="YouTube — description"
        )
    )
    choice = decisions.confirm_chapters(path, chapters)
    if choice == ChapterDecision.EDIT:
        chapters = decisions.edit_chapters(chapters) or []
    if not chapters:
        return None
    path.with_suffix(".chapters.txt").write_text(
        serialize_chapters(chapters), encoding="utf-8"
    )
    return chapters


def _soulseek_ready() -> bool:
    try:
        state = SoulseekSource().check()
    except Exception:
        return False
    return bool(state.get("auth_valid") and state.get("server_connected"))


def _prepopulate_archive(archive: Path) -> None:
    archive.parent.mkdir(parents=True, exist_ok=True)
    prepopulate_archive(archive)

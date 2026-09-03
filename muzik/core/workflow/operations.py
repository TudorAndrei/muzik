"""Concrete core operations for command-line and desktop workflow adapters."""

from __future__ import annotations

import json
from pathlib import Path
import tempfile
from typing import cast

from muzik.core.audio import extract_metadata, get_duration
from muzik.core.quality import QualityPolicy
from muzik.core.beets.decisions import BeetsDecisions, NonInteractiveBeetsDecisions
from muzik.core.beets.events import (
    BeetsErrorEvent,
    BeetsEventEmitter,
    NullBeetsEventEmitter,
)
from muzik.core.beets.importer import ImportOptions
from muzik.core.beets.service import organize_paths, tag_only_with_beet
from muzik.core.chapters import (
    Chapter,
    find_chapters,
    parse_chapters_json,
    serialize_chapters,
    sidecar_path,
)
from muzik.core.description_chapters import (
    description_has_timestamps,
    get_description_from_info_json,
)
from muzik.core.tracklist import chapters_from_comments, chapters_from_description
from muzik.core.sources.youtube import video_id_from_path
from muzik.core.musicbrainz import MIN_ALBUM_DURATION, lookup_chapters_verbose
from muzik.core.metadata_repair import repair_placeholder_album_tags
from muzik.core.sources.base import Candidate
from muzik.core.sources.seakarr import SeakarrSource
from muzik.core.sources.youtube import (
    YouTubeSource,
    dump_json,
    get_playlist_video_ids,
    prepopulate_archive,
)
from muzik.core.splitter import SplitError, split_audio
from muzik.core.workflow.cancellation import CancellationToken, WorkflowCancelled
from muzik.core.workflow.decisions import WorkflowDecisions
from muzik.core.workflow.events import (
    ChapterReviewRequestedEvent,
    MessageEvent,
    NullWorkflowEventEmitter,
    WorkflowEventEmitter,
)
from muzik.core.workflow.decisions import ChapterDecision
from muzik.core.workflow.item_actions import ItemActionOperations
from muzik.core.workflow.service import (
    AudioFallback,
    SplitTask,
    WorkflowOptions,
    WorkflowRequest,
    WorkflowRunOperations,
    WorkflowServiceError,
    MetadataWorkflowSource,
    SoulseekWorkflowSource,
    acquire_from_soulseek,
    acquire_track_from_soulseek,
    check_youtube_quality,
    find_audio_inputs,
    process_audio_plan,
    run_workflow,
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
    """Build concrete operations without binding to an interface toolkit."""
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
                # Under force, ignore the archive so a known id is re-fetched.
                archive_file=None if options.force else archive_file,
                force=options.force,
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
                repair = repair_placeholder_album_tags(target)
                if repair.updated_files:
                    events.emit(
                        MessageEvent(
                            "Repaired placeholder tags for "
                            f"{repair.updated_files} track(s): "
                            f"{repair.artist} - {repair.album} ({repair.year})."
                        )
                    )
                organize_paths(
                    ImportOptions(
                        paths=[target],
                        config_path=options.config
                        if options.config and options.config.exists()
                        else None,
                        move=True,
                        dry_run=options.dry_run,
                        # --force re-imports the same split dir (incremental
                        # would skip it) and replaces the existing album.
                        incremental=not options.force,
                        duplicate_action="remove" if options.force else None,
                    ),
                    tag_only=options.tag_only,
                    decisions=beets_decisions,
                    events=beets_events,
                    tag_only_runner=tag_only_with_beet if options.tag_only else None,
                )
            except WorkflowCancelled:
                raise
            except Exception as exc:
                message = str(exc) or type(exc).__name__
                beets_events.emit(
                    BeetsErrorEvent(message, context={"path": str(target)})
                )
                raise WorkflowServiceError(
                    f"Beets could not organize {target.name}: {message}"
                ) from exc
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
            source_factory=lambda: cast(SoulseekWorkflowSource, SeakarrSource()),
            youtube_source_factory=lambda: cast(
                MetadataWorkflowSource, YouTubeSource()
            ),
            cancellation=cancellation,
        ),
        prepopulate_archive=lambda archive: _prepopulate_archive(archive),
        get_playlist_video_ids=get_playlist_video_ids,
        soulseek_ready=_soulseek_ready,
        acquire_soulseek_track=lambda track, *, cancellation=None: (
            acquire_track_from_soulseek(
                track,
                prefer=options.prefer,
                decisions=decisions,
                events=events,
                source_factory=lambda: cast(SoulseekWorkflowSource, SeakarrSource()),
                cancellation=cancellation,
            )
        ),
        check_quality=lambda audio_files, *, cancellation=None: check_youtube_quality(
            audio_files,
            policy=QualityPolicy(options.quality_policy),
            min_bitrate=options.min_bitrate,
            prefer=options.prefer,
            decisions=decisions,
            events=events,
            source_factory=lambda: cast(SoulseekWorkflowSource, SeakarrSource()),
            cancellation=cancellation,
        ),
    )


def build_item_action_operations(
    *,
    decisions: WorkflowDecisions,
    events: WorkflowEventEmitter | None = None,
    beets_decisions: BeetsDecisions | None = None,
    beets_events: BeetsEventEmitter | None = None,
) -> ItemActionOperations:
    """Build targeted item actions from the normal workflow operations."""
    events = events or NullWorkflowEventEmitter()

    def run_action(
        request: WorkflowRequest,
        options: WorkflowOptions,
        cancellation: CancellationToken,
    ) -> None:
        operations = build_workflow_operations(
            splits=request.splits,
            options=options,
            decisions=decisions,
            events=events,
            beets_decisions=beets_decisions,
            beets_events=beets_events,
        )
        run_workflow(
            request,
            options,
            operations=operations,
            events=events,
            cancellation=cancellation,
        )

    def check_quality_action(
        audio: Path,
        options: WorkflowOptions,
        cancellation: CancellationToken,
    ):
        # An explicit "check quality again" click always actually checks —
        # a global "off" policy would otherwise make this button a silent
        # no-op, which is not what a user asking for it right now expects.
        policy = QualityPolicy(options.quality_policy)
        if policy == QualityPolicy.OFF:
            policy = QualityPolicy.ASK
        return check_youtube_quality(
            [audio],
            policy=policy,
            min_bitrate=options.min_bitrate,
            prefer=options.prefer,
            decisions=decisions,
            events=events,
            source_factory=lambda: cast(SoulseekWorkflowSource, SeakarrSource()),
            cancellation=cancellation,
        )

    return ItemActionOperations(
        run_workflow=run_action,
        parse_chapters=lambda audio, video_url, cancellation: refresh_youtube_chapters(
            audio,
            video_url,
            decisions=decisions,
            events=events,
            cancellation=cancellation,
        ),
        check_quality=check_quality_action,
    )


def refresh_youtube_chapters(
    path: Path,
    video_url: str,
    *,
    decisions: WorkflowDecisions,
    events: WorkflowEventEmitter | None = None,
    cancellation: CancellationToken | None = None,
) -> Path:
    """Refresh YouTube metadata and replace chapters only after acceptance."""
    events = events or NullWorkflowEventEmitter()
    cancellation = cancellation or CancellationToken()
    cancellation.raise_if_cancelled()
    metadata = dump_json(video_url)
    if metadata is None:
        raise WorkflowServiceError("Unable to refresh YouTube video metadata.")
    info_path = sidecar_path(path, ".info.json")
    _atomic_write_text(
        info_path,
        json.dumps(metadata, indent=2, ensure_ascii=False) + "\n",
    )
    cancellation.raise_if_cancelled()

    chapters = parse_chapters_json(info_path)
    if chapters:
        events.emit(
            ChapterReviewRequestedEvent(
                source=path,
                chapters=chapters,
                title="YouTube chapters",
            )
        )
        choice = decisions.confirm_chapters(path, chapters)
        if choice == ChapterDecision.EDIT:
            chapters = decisions.edit_chapters(chapters) or []
        if not chapters or choice == ChapterDecision.REJECT:
            raise WorkflowServiceError("YouTube chapters were not accepted.")
        chapter_path = sidecar_path(path, ".chapters.txt")
        _atomic_write_text(chapter_path, serialize_chapters(chapters))
        return chapter_path

    chapters = _description_chapters(path, decisions, events)
    if not chapters:
        raise WorkflowServiceError("No YouTube chapters were found.")
    cancellation.raise_if_cancelled()
    return sidecar_path(path, ".chapters.txt")


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
    if choice == ChapterDecision.REJECT or not chapters:
        return None
    _atomic_write_text(
        sidecar_path(path, ".chapters.txt"), serialize_chapters(chapters)
    )
    return chapters


def _description_chapters(
    path: Path,
    decisions: WorkflowDecisions,
    events: WorkflowEventEmitter,
) -> list[Chapter] | None:
    info_path = sidecar_path(path, ".info.json")
    if not info_path.exists():
        return None
    log = lambda message: events.emit(MessageEvent(message, severity="debug"))  # noqa: E731
    description = get_description_from_info_json(info_path)
    chapters: list[Chapter] | None = None
    if description and description_has_timestamps(description):
        chapters = chapters_from_description(description, log=log)
    if not chapters:
        # No tracklist in the description; try the pinned/uploader comment.
        video_id = video_id_from_path(path)
        if video_id:
            chapters = chapters_from_comments(video_id, log=log)
    if not chapters:
        return None
    events.emit(
        ChapterReviewRequestedEvent(
            source=path, chapters=chapters, title="YouTube — description"
        )
    )
    choice = decisions.confirm_chapters(path, chapters)
    if choice == ChapterDecision.EDIT:
        chapters = decisions.edit_chapters(chapters) or []
    if choice == ChapterDecision.REJECT or not chapters:
        return None
    _atomic_write_text(
        sidecar_path(path, ".chapters.txt"), serialize_chapters(chapters)
    )
    return chapters


def _atomic_write_text(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="w",
            encoding="utf-8",
            dir=path.parent,
            prefix=f".{path.name}.",
            suffix=".tmp",
            delete=False,
        ) as handle:
            temporary = Path(handle.name)
            handle.write(text)
            handle.flush()
        temporary.replace(path)
    finally:
        if temporary is not None and temporary.exists():
            temporary.unlink()


def _soulseek_ready() -> bool:
    try:
        state = SeakarrSource().check()
    except Exception:
        return False
    return bool(state.get("connected"))


def _prepopulate_archive(archive: Path) -> None:
    archive.parent.mkdir(parents=True, exist_ok=True)
    prepopulate_archive(archive)

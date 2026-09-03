"""Targeted workflow actions for one watchlist video."""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass, replace
from datetime import datetime
from enum import Enum
from pathlib import Path

from muzik.core.chapters import find_chapters
from muzik.core.quality import QualityPolicy
from muzik.core.sources.youtube import find_audio_by_id
from muzik.core.watchlist import StageRecord, StageStatus, WatchlistItem
from muzik.core.workflow.cancellation import CancellationToken, WorkflowCancelled
from muzik.core.workflow.service import (
    QualityUpgradeResult,
    WorkflowOptions,
    WorkflowRequest,
)


class ItemAction(str, Enum):
    RUN = "run"
    RETRY = "retry"
    DOWNLOAD_AGAIN = "download_again"
    CHECK_QUALITY_AGAIN = "check_quality_again"
    PARSE_AGAIN = "parse_again"
    SPLIT_AGAIN = "split_again"
    ORGANIZE_AGAIN = "organize_again"
    RUN_ALL_AGAIN = "run_all_again"


class ItemActionError(RuntimeError):
    """Raised when a card action cannot complete."""


@dataclass(frozen=True, slots=True)
class ActionAvailability:
    enabled: bool
    reason: str | None = None


@dataclass(frozen=True, slots=True)
class ItemActionOperations:
    run_workflow: Callable[[WorkflowRequest, WorkflowOptions, CancellationToken], None]
    parse_chapters: Callable[[Path, str, CancellationToken], Path]
    check_quality: Callable[
        [Path, WorkflowOptions, CancellationToken], QualityUpgradeResult
    ]


@dataclass(frozen=True, slots=True)
class ItemActionResult:
    action: ItemAction
    completed_stage: str


StateCallback = Callable[[], None]


def item_summary_state(item: WatchlistItem) -> str:
    """Return the short state label shown on a watchlist card."""
    if not item.video_id:
        return "Unavailable"
    statuses = {record.status for record in item.stages.values()}
    if StageStatus.RUNNING in statuses:
        return "Processing"
    if StageStatus.FAILED in statuses:
        return "Failed"
    if all(
        record.status in {StageStatus.COMPLETE, StageStatus.SKIPPED}
        for record in item.stages.values()
    ):
        return "Processed"
    return "Pending"


def primary_item_action(item: WatchlistItem) -> tuple[ItemAction | None, str]:
    """Return the primary card command and its user-facing label."""
    if not item.video_id:
        return None, "Unavailable"
    if any(record.status == StageStatus.FAILED for record in item.stages.values()):
        return ItemAction.RETRY, "Retry"
    if any(record.status == StageStatus.STALE for record in item.stages.values()):
        return ItemAction.RUN, "Resume"
    if item_summary_state(item) == "Processed":
        return None, "Processed"
    return ItemAction.RUN, "Run"


def item_action_availability(
    item: WatchlistItem,
    action: ItemAction,
    *,
    request: WorkflowRequest,
) -> ActionAvailability:
    """Return whether an item command has the local inputs it needs."""
    if not item.video_id or not item.video_url:
        return ActionAvailability(False, "This playlist item is unavailable.")
    if action in {
        ItemAction.RUN,
        ItemAction.RETRY,
        ItemAction.DOWNLOAD_AGAIN,
        ItemAction.RUN_ALL_AGAIN,
    }:
        return ActionAvailability(True)

    if action == ItemAction.ORGANIZE_AGAIN:
        target = _organize_target(item, request)
        if target is None or not target.exists():
            return ActionAvailability(
                False,
                "No downloaded audio or split directory is available.",
            )
        return ActionAvailability(True)

    audio = _audio_path(item, request)
    if audio is None:
        return ActionAvailability(
            False,
            "Download this video before you run this command.",
        )
    if action == ItemAction.PARSE_AGAIN:
        return ActionAvailability(True)
    if action == ItemAction.CHECK_QUALITY_AGAIN:
        return ActionAvailability(True)
    if action == ItemAction.SPLIT_AGAIN:
        if not find_chapters(audio):
            return ActionAvailability(
                False,
                "Parse and accept chapters before you split this video.",
            )
        return ActionAvailability(True)
    return ActionAvailability(False, "This command is not available.")


def run_item_action(
    item: WatchlistItem,
    action: ItemAction,
    *,
    request: WorkflowRequest,
    options: WorkflowOptions,
    operations: ItemActionOperations,
    cancellation: CancellationToken | None = None,
    on_state_change: StateCallback | None = None,
) -> ItemActionResult:
    """Run one targeted item command and update its durable stage state."""
    available = item_action_availability(item, action, request=request)
    if not available.enabled:
        raise ItemActionError(available.reason or "This command is not available.")
    token = cancellation or CancellationToken()
    notify = on_state_change or (lambda: None)
    target_stage = _target_stage(item, action)
    previous = replace(item.stages[target_stage])
    item.last_action = action.value
    item.last_error = None
    item.stages[target_stage] = StageRecord(
        status=StageStatus.RUNNING,
        updated_at=_now(),
    )
    notify()

    try:
        _run_action(
            item,
            action,
            request=request,
            options=options,
            operations=operations,
            cancellation=token,
        )
    except WorkflowCancelled:
        item.stages[target_stage] = previous
        notify()
        raise
    except Exception as exc:
        message = str(exc) or f"{action.value} failed."
        item.last_error = message
        item.stages[target_stage] = StageRecord(
            status=StageStatus.FAILED,
            updated_at=_now(),
            error=message,
        )
        notify()
        if isinstance(exc, ItemActionError):
            raise
        raise ItemActionError(message) from exc

    notify()
    return ItemActionResult(action=action, completed_stage=target_stage)


def _run_action(
    item: WatchlistItem,
    action: ItemAction,
    *,
    request: WorkflowRequest,
    options: WorkflowOptions,
    operations: ItemActionOperations,
    cancellation: CancellationToken,
) -> None:
    assert item.video_url is not None
    if action in {ItemAction.RUN, ItemAction.RETRY}:
        operations.run_workflow(
            replace(request, raw=item.video_url),
            replace(options, force=False),
            cancellation,
        )
        _mark_full_workflow_complete(item, options=options)
        return
    if action == ItemAction.DOWNLOAD_AGAIN:
        operations.run_workflow(
            replace(request, raw=item.video_url),
            replace(
                options,
                force=True,
                no_split=True,
                no_organize=True,
            ),
            cancellation,
        )
        local = find_audio_by_id(request.output, item.video_id or "")
        item.stages["download"] = StageRecord(
            status=StageStatus.COMPLETE,
            updated_at=_now(),
            path=str(local[0].resolve()) if local else None,
        )
        _mark_stale(item, "parse", "split", "organize")
        return

    if action == ItemAction.ORGANIZE_AGAIN:
        target = _organize_target(item, request)
        if target is None:
            raise ItemActionError(
                "No downloaded audio or split directory is available."
            )
        operations.run_workflow(
            replace(request, raw=str(target)),
            replace(options, force=True, no_split=True, no_organize=False),
            cancellation,
        )
        item.stages["organize"] = StageRecord(
            status=StageStatus.COMPLETE,
            updated_at=_now(),
        )
        return

    audio = _audio_path(item, request)
    if audio is None:
        raise ItemActionError("Downloaded audio is not available.")
    if action == ItemAction.PARSE_AGAIN:
        chapter_path = operations.parse_chapters(audio, item.video_url, cancellation)
        item.stages["parse"] = StageRecord(
            status=StageStatus.COMPLETE,
            updated_at=_now(),
            path=str(chapter_path.resolve()),
        )
        _mark_stale(item, "split", "organize")
        return
    if action == ItemAction.CHECK_QUALITY_AGAIN:
        result = operations.check_quality(audio, options, cancellation)
        if result.pre_split_dirs:
            # A multi-file replacement changes what "download" even means for
            # this item (a directory, not one file) — later stages restart.
            item.stages["download"] = StageRecord(
                status=StageStatus.COMPLETE,
                updated_at=_now(),
                path=str(result.pre_split_dirs[0]),
            )
            item.stages["quality"] = StageRecord(
                status=StageStatus.COMPLETE, updated_at=_now()
            )
            _mark_stale(item, "parse", "split", "organize")
            return
        replacement = result.audio_files[0] if result.audio_files else audio
        item.stages["quality"] = StageRecord(
            status=StageStatus.COMPLETE,
            updated_at=_now(),
            path=str(replacement.resolve()) if result.replaced else None,
        )
        if result.replaced:
            # The active audio file changed: parse/split/organize ran against
            # the old one and no longer reflect what will be imported.
            item.stages["download"] = StageRecord(
                status=StageStatus.COMPLETE,
                updated_at=_now(),
                path=str(replacement.resolve()),
            )
            _mark_stale(item, "parse", "split", "organize")
        return
    if action == ItemAction.SPLIT_AGAIN:
        operations.run_workflow(
            replace(request, raw=str(audio)),
            replace(options, force=True, no_split=False, no_organize=True),
            cancellation,
        )
        split_dir = request.splits / audio.stem
        if not split_dir.is_dir():
            raise ItemActionError("Split did not create an output directory.")
        item.stages["split"] = StageRecord(
            status=StageStatus.COMPLETE,
            updated_at=_now(),
            path=str(split_dir.resolve()),
        )
        _mark_stale(item, "organize")
        return
    if action == ItemAction.RUN_ALL_AGAIN:
        operations.run_workflow(
            replace(request, raw=item.video_url),
            replace(options, force=True),
            cancellation,
        )
        _mark_full_workflow_complete(item, options=options)
        return
    raise ItemActionError("This command is not available.")


def _target_stage(item: WatchlistItem, action: ItemAction) -> str:
    if action in {ItemAction.DOWNLOAD_AGAIN, ItemAction.RUN_ALL_AGAIN}:
        return "download"
    if action == ItemAction.CHECK_QUALITY_AGAIN:
        return "quality"
    if action == ItemAction.PARSE_AGAIN:
        return "parse"
    if action == ItemAction.SPLIT_AGAIN:
        return "split"
    if action == ItemAction.ORGANIZE_AGAIN:
        return "organize"
    for name in ("download", "quality", "parse", "split", "organize"):
        if item.stages[name].status not in {
            StageStatus.COMPLETE,
            StageStatus.SKIPPED,
        }:
            return name
    return "download"


def _audio_path(item: WatchlistItem, request: WorkflowRequest) -> Path | None:
    saved = item.stages["download"].path
    if saved:
        path = Path(saved)
        if path.is_file():
            return path
    if item.video_id:
        local = find_audio_by_id(request.output, item.video_id)
        if local:
            return local[0]
    return None


def _organize_target(
    item: WatchlistItem,
    request: WorkflowRequest,
) -> Path | None:
    split_path = item.stages["split"].path
    if split_path:
        split = Path(split_path)
        if split.exists():
            return split
    return _audio_path(item, request)


def _mark_stale(item: WatchlistItem, *names: str) -> None:
    for name in names:
        item.stages[name] = StageRecord(
            status=StageStatus.STALE,
            updated_at=_now(),
        )


def _mark_full_workflow_complete(
    item: WatchlistItem,
    *,
    options: WorkflowOptions,
) -> None:
    item.last_error = None
    item.stages["download"].status = StageStatus.COMPLETE
    item.stages["download"].updated_at = _now()
    item.stages["quality"].status = (
        StageStatus.SKIPPED
        if QualityPolicy(options.quality_policy) == QualityPolicy.OFF
        else StageStatus.COMPLETE
    )
    item.stages["quality"].updated_at = _now()
    item.stages["parse"].status = (
        StageStatus.SKIPPED if options.no_split else StageStatus.COMPLETE
    )
    item.stages["parse"].updated_at = _now()
    item.stages["split"].status = StageStatus.SKIPPED
    item.stages["split"].updated_at = _now()
    item.stages["organize"].status = (
        StageStatus.SKIPPED if options.no_organize else StageStatus.COMPLETE
    )
    item.stages["organize"].updated_at = _now()


def _now() -> str:
    return datetime.now().astimezone().isoformat(timespec="seconds")

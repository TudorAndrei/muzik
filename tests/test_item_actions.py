from pathlib import Path

import pytest

from muzik.core.chapters import sidecar_path
from muzik.core.watchlist import StageRecord, StageStatus, WatchlistItem
from muzik.core.workflow import operations as workflow_operations
from muzik.core.workflow.cancellation import WorkflowCancelled
from muzik.core.workflow.decisions import ChapterDecision
from muzik.core.workflow.item_actions import (
    ItemAction,
    ItemActionError,
    ItemActionOperations,
    item_action_availability,
    item_summary_state,
    primary_item_action,
    run_item_action,
)
from muzik.core.workflow.service import (
    WorkflowOptions,
    WorkflowRequest,
    WorkflowServiceError,
)


def _item() -> WatchlistItem:
    return WatchlistItem(
        position=1,
        title="Mix",
        video_id="abcdefghijk",
        video_url="https://www.youtube.com/watch?v=abcdefghijk",
    )


def _request(tmp_path: Path) -> WorkflowRequest:
    return WorkflowRequest(
        raw="watchlist",
        output=tmp_path / "downloads",
        splits=tmp_path / "splits",
    )


def test_download_again_forces_only_download_and_marks_later_stages_stale(
    tmp_path: Path,
) -> None:
    item = _item()
    for stage in item.stages.values():
        stage.status = StageStatus.COMPLETE
    calls = []
    state_changes = 0

    def run(request, options, cancellation):
        calls.append((request, options))
        request.output.mkdir(parents=True)
        (request.output / "Mix [abcdefghijk].m4a").write_bytes(b"audio")

    def changed() -> None:
        nonlocal state_changes
        state_changes += 1

    run_item_action(
        item,
        ItemAction.DOWNLOAD_AGAIN,
        request=_request(tmp_path),
        options=WorkflowOptions(),
        operations=ItemActionOperations(run, lambda *args: Path("unused")),
        on_state_change=changed,
    )

    action_options = calls[0][1]
    assert action_options.force is True
    assert action_options.no_split is True
    assert action_options.no_organize is True
    assert item.stages["download"].status is StageStatus.COMPLETE
    assert item.stages["download"].path is not None
    assert [item.stages[name].status for name in ("parse", "split", "organize")] == [
        StageStatus.STALE,
        StageStatus.STALE,
        StageStatus.STALE,
    ]
    assert state_changes == 2


def test_parse_again_marks_later_stages_stale(tmp_path: Path) -> None:
    item = _item()
    audio = tmp_path / "Mix [abcdefghijk].m4a"
    audio.write_bytes(b"audio")
    item.stages["download"] = StageRecord(status=StageStatus.COMPLETE, path=str(audio))
    item.stages["split"].status = StageStatus.COMPLETE
    item.stages["organize"].status = StageStatus.COMPLETE
    chapter_path = sidecar_path(audio, ".chapters.txt")

    def parse(path, url, cancellation):
        chapter_path.write_text("00:00 First\n", encoding="utf-8")
        return chapter_path

    run_item_action(
        item,
        ItemAction.PARSE_AGAIN,
        request=_request(tmp_path),
        options=WorkflowOptions(),
        operations=ItemActionOperations(lambda *args: None, parse),
    )

    assert item.stages["parse"].path == str(chapter_path.resolve())
    assert item.stages["split"].status is StageStatus.STALE
    assert item.stages["organize"].status is StageStatus.STALE


def test_split_again_requires_chapters_and_marks_organize_stale(
    tmp_path: Path,
) -> None:
    item = _item()
    audio = tmp_path / "Mix [abcdefghijk].m4a"
    audio.write_bytes(b"audio")
    item.stages["download"] = StageRecord(status=StageStatus.COMPLETE, path=str(audio))
    request = _request(tmp_path)

    disabled = item_action_availability(item, ItemAction.SPLIT_AGAIN, request=request)
    assert disabled.enabled is False
    assert disabled.reason == "Parse and accept chapters before you split this video."

    sidecar_path(audio, ".chapters.txt").write_text(
        "00:00 First\n01:00 Second\n", encoding="utf-8"
    )

    def run(action_request, options, cancellation):
        split_dir = request.splits / audio.stem
        split_dir.mkdir(parents=True)

    run_item_action(
        item,
        ItemAction.SPLIT_AGAIN,
        request=request,
        options=WorkflowOptions(),
        operations=ItemActionOperations(run, lambda *args: Path("unused")),
    )

    assert item.stages["split"].status is StageStatus.COMPLETE
    assert item.stages["organize"].status is StageStatus.STALE


def test_organize_again_uses_split_directory(tmp_path: Path) -> None:
    item = _item()
    audio = tmp_path / "Mix [abcdefghijk].m4a"
    split_dir = tmp_path / "splits" / "Mix"
    split_dir.mkdir(parents=True)
    item.stages["download"] = StageRecord(status=StageStatus.COMPLETE, path=str(audio))
    item.stages["split"] = StageRecord(status=StageStatus.COMPLETE, path=str(split_dir))
    calls = []

    def run(request, options, cancellation):
        calls.append((request, options))

    run_item_action(
        item,
        ItemAction.ORGANIZE_AGAIN,
        request=_request(tmp_path),
        options=WorkflowOptions(),
        operations=ItemActionOperations(run, lambda *args: Path("unused")),
    )

    assert calls[0][0].raw == str(split_dir)
    assert calls[0][1].force is True
    assert calls[0][1].no_split is True
    assert item.stages["organize"].status is StageStatus.COMPLETE


def test_item_action_failure_is_saved_and_retry_becomes_primary(tmp_path: Path) -> None:
    item = _item()
    saved_states = []

    def fail(request, options, cancellation):
        raise RuntimeError("network stopped")

    with pytest.raises(ItemActionError, match="network stopped"):
        run_item_action(
            item,
            ItemAction.RUN,
            request=_request(tmp_path),
            options=WorkflowOptions(),
            operations=ItemActionOperations(fail, lambda *args: Path("unused")),
            on_state_change=lambda: saved_states.append(item.stages["download"].status),
        )

    assert item.stages["download"].status is StageStatus.FAILED
    assert item.last_error == "network stopped"
    assert saved_states == [StageStatus.RUNNING, StageStatus.FAILED]
    assert primary_item_action(item) == (ItemAction.RETRY, "Retry")


def test_item_action_cancellation_restores_previous_stage(tmp_path: Path) -> None:
    item = _item()
    item.stages["download"].status = StageStatus.STALE

    def cancel(request, options, cancellation):
        raise WorkflowCancelled("cancelled")

    with pytest.raises(WorkflowCancelled):
        run_item_action(
            item,
            ItemAction.RUN,
            request=_request(tmp_path),
            options=WorkflowOptions(),
            operations=ItemActionOperations(cancel, lambda *args: Path("unused")),
        )

    assert item.stages["download"].status is StageStatus.STALE


def test_unavailable_and_missing_audio_actions_have_reasons(tmp_path: Path) -> None:
    unavailable = WatchlistItem(1, "Private video", None)
    request = _request(tmp_path)

    assert item_summary_state(unavailable) == "Unavailable"
    assert (
        item_action_availability(
            unavailable, ItemAction.DOWNLOAD_AGAIN, request=request
        ).reason
        == "This playlist item is unavailable."
    )
    assert (
        item_action_availability(
            _item(), ItemAction.PARSE_AGAIN, request=request
        ).reason
        == "Download this video before you run this command."
    )


class RejectChapters:
    def confirm_chapters(self, source, chapters):
        return ChapterDecision.REJECT

    def edit_chapters(self, chapters):
        return chapters

    def choose_soulseek_candidate(self, candidates):
        return candidates[0]


class AcceptChapters(RejectChapters):
    def confirm_chapters(self, source, chapters):
        return ChapterDecision.ACCEPT


def test_refresh_youtube_chapters_preserves_old_sidecar_on_rejection(
    tmp_path: Path,
    monkeypatch,
) -> None:
    audio = tmp_path / "Mix [abcdefghijk].m4a"
    audio.write_bytes(b"audio")
    chapter_path = sidecar_path(audio, ".chapters.txt")
    chapter_path.write_text("00:00 Old chapter\n", encoding="utf-8")
    monkeypatch.setattr(
        workflow_operations,
        "dump_json",
        lambda url: {
            "chapters": [{"start_time": 0, "end_time": 60, "title": "New chapter"}]
        },
    )

    with pytest.raises(WorkflowServiceError, match="not accepted"):
        workflow_operations.refresh_youtube_chapters(
            audio,
            "https://youtube.com/watch?v=abcdefghijk",
            decisions=RejectChapters(),
        )

    assert chapter_path.read_text(encoding="utf-8") == "00:00 Old chapter\n"


def test_refresh_youtube_chapters_replaces_sidecar_after_acceptance(
    tmp_path: Path,
    monkeypatch,
) -> None:
    audio = tmp_path / "Mix [abcdefghijk].m4a"
    audio.write_bytes(b"audio")
    chapter_path = sidecar_path(audio, ".chapters.txt")
    chapter_path.write_text("00:00 Old chapter\n", encoding="utf-8")
    monkeypatch.setattr(
        workflow_operations,
        "dump_json",
        lambda url: {
            "chapters": [{"start_time": 0, "end_time": 60, "title": "New chapter"}]
        },
    )

    result = workflow_operations.refresh_youtube_chapters(
        audio,
        "https://youtube.com/watch?v=abcdefghijk",
        decisions=AcceptChapters(),
    )

    assert result == chapter_path
    assert chapter_path.read_text(encoding="utf-8") == "00:00:00 New chapter\n"


def test_refresh_description_chapters_preserves_sidecar_on_rejection(
    tmp_path: Path,
    monkeypatch,
) -> None:
    audio = tmp_path / "Mix [abcdefghijk].m4a"
    audio.write_bytes(b"audio")
    chapter_path = sidecar_path(audio, ".chapters.txt")
    chapter_path.write_text("00:00 Old chapter\n", encoding="utf-8")
    monkeypatch.setattr(
        workflow_operations,
        "dump_json",
        lambda url: {
            "description": "00:00 New first\n01:00 New second",
            "chapters": [],
        },
    )

    with pytest.raises(WorkflowServiceError, match="No YouTube chapters were found"):
        workflow_operations.refresh_youtube_chapters(
            audio,
            "https://youtube.com/watch?v=abcdefghijk",
            decisions=RejectChapters(),
        )

    assert chapter_path.read_text(encoding="utf-8") == "00:00 Old chapter\n"

from pathlib import Path

import pytest

from muzik.commands import workflow
from muzik.core.chapters import Chapter
from muzik.core.sources.base import Candidate, DownloadResult, ResolvedRelease
from muzik.core.workflow.decisions import (
    ChapterDecision,
    NonInteractiveWorkflowDecisions,
    WorkflowDecisionError,
)
from muzik.core.workflow.events import (
    CandidatesFoundEvent,
    ChapterReviewRequestedEvent,
    MessageEvent,
    RecordingWorkflowEventEmitter,
    StepFinishedEvent,
    StepStartedEvent,
)
from muzik.ui.chapter_editor import edit_chapters
from muzik.ui.cli.decisions import CliWorkflowDecisions


def _candidate(source_id: str, title: str = "Album") -> Candidate:
    return Candidate(
        source="soulseek",
        source_id=source_id,
        title=title,
        score=100,
    )


def test_noninteractive_decisions_choose_candidate_without_prompt() -> None:
    decisions = NonInteractiveWorkflowDecisions(candidate_index=1)

    selected = decisions.choose_soulseek_candidate(
        [_candidate("first"), _candidate("second")]
    )

    assert selected.source_id == "second"


def test_noninteractive_decisions_reject_out_of_range_candidate() -> None:
    decisions = NonInteractiveWorkflowDecisions(candidate_index=2)

    with pytest.raises(WorkflowDecisionError):
        decisions.choose_soulseek_candidate([_candidate("first")])


def test_cli_decisions_can_use_injected_candidate_prompt() -> None:
    prompts: list[tuple[str, str]] = []

    def prompt(label: str, *, default: str) -> str:
        prompts.append((label, default))
        return "2"

    decisions = CliWorkflowDecisions(
        candidate_limit=2,
        prompt=prompt,
        display_soulseek_candidates=False,
    )

    selected = decisions.choose_soulseek_candidate(
        [_candidate("first"), _candidate("second")]
    )

    assert selected.source_id == "second"
    assert prompts == [("  Soulseek candidate", "1")]


def test_get_chapters_for_accepts_musicbrainz_without_input(
    tmp_path: Path,
    monkeypatch,
) -> None:
    audio = tmp_path / "Album.flac"
    audio.write_bytes(b"")
    chapters = [
        Chapter(index=1, start=0, end=60, title="One"),
        Chapter(index=2, start=60, end=None, title="Two"),
    ]

    monkeypatch.setattr(workflow, "find_chapters", lambda path: [])
    monkeypatch.setattr(workflow, "get_duration", lambda path: 3600)
    monkeypatch.setattr(
        workflow,
        "extract_metadata",
        lambda path: {"artist": "Artist", "album": "Album", "year": "2024"},
    )
    monkeypatch.setattr(
        workflow,
        "lookup_chapters",
        lambda artist, album, year: (chapters, "Album", ""),
    )

    decisions = NonInteractiveWorkflowDecisions(chapter_decision=ChapterDecision.ACCEPT)
    events = RecordingWorkflowEventEmitter()

    selected = workflow._get_chapters_for(
        audio,
        no_split=False,
        decisions=decisions,
        events=events,
    )

    assert selected == chapters
    assert events.events == [
        ChapterReviewRequestedEvent(
            source=audio,
            chapters=chapters,
            title="MusicBrainz — Album",
        )
    ]
    assert audio.with_suffix(".chapters.txt").read_text(encoding="utf-8") == (
        "00:00:00 One\n00:01:00 Two\n"
    )


def test_get_chapters_for_rejects_musicbrainz_without_input(
    tmp_path: Path,
    monkeypatch,
) -> None:
    audio = tmp_path / "Album.flac"
    audio.write_bytes(b"")
    chapters = [Chapter(index=1, start=0, end=None, title="One")]

    monkeypatch.setattr(workflow, "find_chapters", lambda path: [])
    monkeypatch.setattr(workflow, "get_duration", lambda path: 3600)
    monkeypatch.setattr(
        workflow,
        "extract_metadata",
        lambda path: {"artist": "Artist", "album": "Album", "year": "2024"},
    )
    monkeypatch.setattr(
        workflow,
        "lookup_chapters",
        lambda artist, album, year: (chapters, "Album", ""),
    )

    decisions = NonInteractiveWorkflowDecisions(chapter_decision=ChapterDecision.REJECT)

    selected = workflow._get_chapters_for(audio, no_split=False, decisions=decisions)

    assert selected is None
    assert not audio.with_suffix(".chapters.txt").exists()


def test_edit_chapters_can_continue_without_input() -> None:
    chapters = [Chapter(index=1, start=0, end=None, title="One")]

    class Decisions:
        def choose_action(self, chapters):
            return ChapterDecision.ACCEPT

    assert edit_chapters(chapters, decisions=Decisions()) == chapters


def test_acquire_from_soulseek_emits_candidates_found(
    tmp_path: Path,
    monkeypatch,
) -> None:
    audio = tmp_path / "01 One.flac"
    audio.write_bytes(b"")
    candidate = _candidate("peer:/Album")

    class FakeSoulseekSource:
        def resolve(self, request):
            return ResolvedRelease(
                title="Album",
                artist="Artist",
                album="Album",
                source="soulseek",
                source_id=request.raw,
            )

        def search(self, resolved, *, prefer, limit):
            return [candidate]

        def download(self, candidate, wait):
            return DownloadResult(
                source="soulseek",
                source_id=candidate.source_id,
                files=[audio],
                root=tmp_path,
            )

    monkeypatch.setattr(workflow, "SoulseekSource", FakeSoulseekSource)
    events = RecordingWorkflowEventEmitter()

    files = workflow._acquire_from_soulseek(
        "Artist - Album",
        prefer="flac",
        interactive=False,
        fallback="none",
        decisions=NonInteractiveWorkflowDecisions(),
        events=events,
    )

    assert files == [audio]
    assert events.events == [
        CandidatesFoundEvent(candidates=[candidate], source="soulseek", limit=10),
        MessageEvent(message="Selected Soulseek candidate: Album"),
        MessageEvent(message="Downloading selected Soulseek candidate."),
        MessageEvent(message="Soulseek download returned 1 file(s)."),
    ]


def test_process_audio_files_emits_organize_steps(
    tmp_path: Path,
    monkeypatch,
) -> None:
    split_dir = tmp_path / "splits" / "Album"
    split_dir.mkdir(parents=True)
    organized: list[Path] = []

    def fake_organize_cmd(**kwargs):
        organized.append(kwargs["directory"])

    monkeypatch.setattr(workflow, "organize_cmd", fake_organize_cmd)
    events = RecordingWorkflowEventEmitter()

    workflow._process_audio_files(
        audio_inputs=[],
        pre_split_dirs=[split_dir],
        splits=tmp_path / "splits",
        review=False,
        no_split=False,
        no_organize=False,
        import_=False,
        tag_only=False,
        dry_run=False,
        jobs=0,
        config=None,
        keep_source=False,
        force=False,
        decisions=NonInteractiveWorkflowDecisions(),
        events=events,
    )

    assert organized == [split_dir]
    assert events.events == [
        StepStartedEvent(name="organize"),
        StepFinishedEvent(name="organize"),
    ]

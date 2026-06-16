from pathlib import Path

import pytest

from muzik.core.workflow import service
from muzik.core import cache as cache_mod
from muzik.core.chapters import Chapter
from muzik.core.sources.base import Candidate, DownloadResult, ResolvedRelease
from muzik.core.workflow.decisions import NonInteractiveWorkflowDecisions
from muzik.core.workflow.events import (
    CandidatesFoundEvent,
    MessageEvent,
    RecordingWorkflowEventEmitter,
)
from muzik.core.workflow.events import StepFinishedEvent, StepStartedEvent


def _candidate(source_id: str = "peer:/Album") -> Candidate:
    return Candidate(
        source="soulseek",
        source_id=source_id,
        title="Album",
        score=100,
    )


def test_find_audio_inputs_recurses_and_deduplicates(tmp_path: Path) -> None:
    album = tmp_path / "album"
    album.mkdir()
    audio = album / "01 One.flac"
    audio.write_bytes(b"audio")
    (album / "cover.jpg").write_bytes(b"image")

    assert service.find_audio_inputs([album, audio]) == [audio]


def test_validated_audio_files_skips_validation_for_dry_run(
    tmp_path: Path,
    monkeypatch,
) -> None:
    audio = tmp_path / "missing.flac"

    def fail_duration(path):
        raise AssertionError("dry-run should not probe audio")

    monkeypatch.setattr(service, "get_duration", fail_duration)

    valid, warnings = service.validated_audio_files(
        [audio],
        dry_run=True,
        no_organize=False,
    )

    assert valid == [audio]
    assert warnings == []


def test_validated_audio_files_returns_warnings_for_rejected_files(
    tmp_path: Path,
    monkeypatch,
) -> None:
    missing = tmp_path / "missing.flac"
    bad = tmp_path / "bad.flac"
    good = tmp_path / "good.flac"
    bad.write_bytes(b"bad")
    good.write_bytes(b"good")

    monkeypatch.setattr(
        service,
        "get_duration",
        lambda path: 180 if path == good else None,
    )

    valid, warnings = service.validated_audio_files(
        [missing, bad, good],
        dry_run=False,
        no_organize=False,
    )

    assert valid == [good]
    assert warnings == [
        f"Skipping missing audio file: {missing}",
        f"Skipping unprobeable audio file: {bad}",
    ]


def test_validated_audio_files_raises_when_all_inputs_are_rejected(
    tmp_path: Path,
    monkeypatch,
) -> None:
    audio = tmp_path / "bad.flac"
    audio.write_bytes(b"bad")
    monkeypatch.setattr(service, "get_duration", lambda path: None)

    with pytest.raises(service.WorkflowServiceError) as exc:
        service.validated_audio_files(
            [audio],
            dry_run=False,
            no_organize=False,
        )

    assert exc.value.exit_code == 1
    assert exc.value.warnings == [f"Skipping unprobeable audio file: {audio}"]


def test_plan_audio_processing_splits_albums_and_singles(tmp_path: Path) -> None:
    album = tmp_path / "Album.flac"
    single = tmp_path / "Single.flac"
    pre_split = tmp_path / "splits" / "Existing"
    chapters = [Chapter(index=1, start=0, end=None, title="One")]

    plan = service.plan_audio_processing(
        [album, single],
        pre_split_dirs=[pre_split],
        chapter_resolver=lambda path: chapters if path == album else None,
    )

    assert plan.albums == [(album, chapters)]
    assert plan.singles == [single]
    assert plan.split_dirs == [pre_split]


def test_organize_targets_for_singles_collapses_common_folder(
    tmp_path: Path,
) -> None:
    album = tmp_path / "Album"
    album.mkdir()
    tracks = [album / "01 One.flac", album / "02 Two.flac"]

    assert service.organize_targets_for_singles(tracks) == [album]


def test_process_audio_plan_runs_split_and_organize(
    tmp_path: Path,
) -> None:
    album = tmp_path / "Album.flac"
    single_dir = tmp_path / "Singles"
    single_dir.mkdir()
    single = single_dir / "Single.flac"
    chapters = [Chapter(index=1, start=0, end=None, title="One")]
    calls: list[str] = []
    events = RecordingWorkflowEventEmitter()

    def split_operation(task: service.SplitTask) -> bool:
        calls.append(f"split:{task.source.name}:{task.output.name}")
        task.output.mkdir(parents=True)
        return True

    def organize_operation(target: Path) -> bool:
        calls.append(f"organize:{target.name}")
        return True

    result = service.process_audio_plan(
        audio_files=[album, single],
        pre_split_dirs=[],
        splits=tmp_path / "splits",
        options=service.WorkflowOptions(),
        chapter_resolver=lambda path: chapters if path == album else None,
        split_operation=split_operation,
        organize_operation=organize_operation,
        events=events,
    )

    assert calls == [
        "split:Album.flac:Album",
        "organize:Album",
        "organize:Single.flac",
    ]
    assert result.split_dirs == [tmp_path / "splits" / "Album"]
    assert result.organize_targets == [
        tmp_path / "splits" / "Album",
        single,
    ]
    assert events.events == [
        StepStartedEvent(name="split", detail="1 album(s)"),
        StepFinishedEvent(name="split", detail="1 output dir(s)"),
        StepStartedEvent(name="organize"),
        StepFinishedEvent(name="organize"),
    ]


def test_process_audio_plan_skips_operations_for_dry_run(
    tmp_path: Path,
) -> None:
    album = tmp_path / "Album.flac"
    chapters = [Chapter(index=1, start=0, end=None, title="One")]

    def fail_split(task: service.SplitTask) -> bool:
        raise AssertionError("dry-run should not split")

    def fail_organize(target: Path) -> bool:
        raise AssertionError("dry-run should not organize")

    result = service.process_audio_plan(
        audio_files=[album],
        pre_split_dirs=[],
        splits=tmp_path / "splits",
        options=service.WorkflowOptions(dry_run=True),
        chapter_resolver=lambda path: chapters,
        split_operation=fail_split,
        organize_operation=fail_organize,
    )

    assert result.split_dirs == []
    assert result.organize_targets == []


def test_process_audio_plan_skips_organize_when_requested(
    tmp_path: Path,
) -> None:
    single = tmp_path / "Single.flac"
    calls: list[str] = []

    def fail_split(task: service.SplitTask) -> bool:
        raise AssertionError("single should not split")

    def fail_organize(target: Path) -> bool:
        raise AssertionError("--no-organize should skip organize")

    class Hooks:
        def albums_detected(self, albums):
            calls.append(f"albums:{len(albums)}")

        def singles_detected(self, singles):
            calls.append(f"singles:{len(singles)}")

        def split_started(self, task, *, dry_run):
            calls.append("split")

        def split_failed(self, source):
            calls.append("split_failed")

        def organize_started(self, target):
            calls.append("organize")

        def complete(self, *, organized):
            calls.append(f"complete:{organized}")

    result = service.process_audio_plan(
        audio_files=[single],
        pre_split_dirs=[],
        splits=tmp_path / "splits",
        options=service.WorkflowOptions(no_organize=True),
        chapter_resolver=lambda path: None,
        split_operation=fail_split,
        organize_operation=fail_organize,
        hooks=Hooks(),
    )

    assert result.organize_targets == []
    assert calls == ["singles:1", "complete:False"]


def test_playlist_state_load_save_round_trip(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")

    state = service.load_playlist_state("PL123")
    state["videos"]["abcdefghijk"] = {"status": "downloaded"}
    service.save_playlist_state("PL123", state)

    loaded = service.load_playlist_state("PL123")

    assert loaded["playlist_id"] == "PL123"
    assert loaded["videos"]["abcdefghijk"] == {"status": "downloaded"}
    assert "last_updated" in loaded


def test_backfill_playlist_entry_from_legacy_split_cache(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    missing_audio = tmp_path / "downloads" / "Album [abcdefghijk].flac"
    split_dir = tmp_path / "splits" / missing_audio.stem
    split_dir.mkdir(parents=True)
    cache_mod.set("yt_abcdefghijk", str(missing_audio))

    entry = service.backfill_playlist_entry_from_legacy_cache(
        "abcdefghijk",
        splits=tmp_path / "splits",
    )

    assert entry == {
        "status": "split",
        "audio_file": str(missing_audio),
        "split_dir": str(split_dir.resolve()),
    }


def test_run_workflow_reuses_legacy_split_cache(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    missing_audio = tmp_path / "downloads" / "Album [abcdefghijk].flac"
    split_dir = tmp_path / "splits" / missing_audio.stem
    split_dir.mkdir(parents=True)
    cache_mod.set("yt_abcdefghijk", str(missing_audio))
    processed: list[tuple[list[Path], list[Path]]] = []

    operations = service.WorkflowRunOperations(
        download_audio=lambda url, output, archive: False,
        process_audio=lambda audio, split: processed.append((audio, split)),
        acquire_soulseek=lambda request: [],
        prepopulate_archive=lambda archive: None,
        get_playlist_video_ids=lambda url: [],
    )

    service.run_workflow(
        service.WorkflowRequest(
            raw="https://youtube.com/watch?v=abcdefghijk",
            output=tmp_path / "downloads",
            splits=tmp_path / "splits",
        ),
        service.WorkflowOptions(audio_source="youtube"),
        operations=operations,
    )

    assert processed == [([], [split_dir])]


def test_acquire_from_soulseek_uses_fake_sources_and_records_cache(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    audio = tmp_path / "01 One.flac"
    audio.write_bytes(b"audio")
    candidate = _candidate()
    calls: list[str] = []

    class FakeSource:
        def resolve(self, request):
            calls.append(f"resolve:{request.raw}:{request.prefer_format}")
            return ResolvedRelease(title="Album", artist="Artist")

        def search(self, resolved, *, prefer, limit):
            calls.append(f"search:{prefer}:{limit}")
            return [candidate]

        def download(self, candidate, wait):
            calls.append(f"download:{candidate.source_id}:{wait}")
            return DownloadResult(
                source="soulseek",
                source_id=candidate.source_id,
                files=[audio],
                root=tmp_path,
            )

    events = RecordingWorkflowEventEmitter()

    files = service.acquire_from_soulseek(
        "Artist - Album",
        prefer="flac",
        fallback="none",
        decisions=NonInteractiveWorkflowDecisions(),
        events=events,
        source_factory=FakeSource,
    )

    assert files == [audio]
    assert calls == [
        "resolve:Artist - Album:flac",
        "search:flac:10",
        "download:peer:/Album:True",
    ]
    assert events.events == [
        CandidatesFoundEvent(candidates=[candidate], source="soulseek", limit=10),
        MessageEvent(message="Selected Soulseek candidate: Album"),
        MessageEvent(message="Downloading selected Soulseek candidate."),
        MessageEvent(message="Soulseek download returned 1 file(s)."),
    ]
    state = cache_mod.get_json(
        cache_mod.workflow_cache_key("soulseek", "Artist - Album")
    )
    assert state is not None
    assert state["source_id"] == "peer:/Album"


def test_acquire_from_soulseek_returns_empty_for_youtube_fallback() -> None:
    class FakeSource:
        def resolve(self, request):
            raise RuntimeError("slskd unavailable")

        def search(self, resolved, *, prefer, limit):
            raise AssertionError("search should not run after resolve failure")

        def download(self, candidate, wait):
            raise AssertionError("download should not run after resolve failure")

    files = service.acquire_from_soulseek(
        "https://youtube.com/watch?v=abcdefghijk",
        prefer="flac",
        fallback="youtube",
        decisions=NonInteractiveWorkflowDecisions(),
        source_factory=FakeSource,
    )

    assert files == []


def test_run_workflow_downloads_single_youtube_and_processes_audio(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    processed: list[tuple[list[Path], list[Path]]] = []
    downloads: list[tuple[str, Path | None]] = []

    def download_audio(url: str, output: Path, archive_file: Path | None) -> bool:
        downloads.append((url, archive_file))
        output.mkdir(parents=True)
        (output / "Downloaded [abcdefghijk].flac").write_bytes(b"audio")
        return True

    operations = service.WorkflowRunOperations(
        download_audio=download_audio,
        process_audio=lambda audio, split: processed.append((audio, split)),
        acquire_soulseek=lambda request: [],
        prepopulate_archive=lambda archive: None,
        get_playlist_video_ids=lambda url: [],
    )

    service.run_workflow(
        service.WorkflowRequest(
            raw="https://youtube.com/watch?v=abcdefghijk",
            output=tmp_path / "downloads",
            splits=tmp_path / "splits",
        ),
        service.WorkflowOptions(audio_source="youtube"),
        operations=operations,
    )

    assert downloads == [("https://youtube.com/watch?v=abcdefghijk", None)]
    assert processed == [
        ([tmp_path / "downloads" / "Downloaded [abcdefghijk].flac"], [])
    ]
    assert cache_mod.get("yt_abcdefghijk") == str(
        tmp_path / "downloads" / "Downloaded [abcdefghijk].flac"
    )


def test_run_workflow_processes_playlist_with_soulseek(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    acquired: list[str] = []
    processed: list[tuple[list[Path], list[Path]]] = []
    archives: list[Path] = []

    def acquire(request: str) -> list[Path]:
        acquired.append(request)
        audio = tmp_path / "downloads" / f"{request[-11:]}.flac"
        audio.parent.mkdir(parents=True, exist_ok=True)
        audio.write_bytes(b"audio")
        return [audio]

    operations = service.WorkflowRunOperations(
        download_audio=lambda url, output, archive: False,
        process_audio=lambda audio, split: processed.append((audio, split)),
        acquire_soulseek=acquire,
        prepopulate_archive=lambda archive: archives.append(archive),
        get_playlist_video_ids=lambda url: ["abcdefghijk", "lmnopqrstuv"],
    )

    service.run_workflow(
        service.WorkflowRequest(
            raw="https://youtube.com/playlist?list=PL123",
            output=tmp_path / "downloads",
            splits=tmp_path / "splits",
        ),
        service.WorkflowOptions(audio_source="soulseek", no_organize=True),
        operations=operations,
    )

    assert archives == [tmp_path / "cache" / "ytdlp_archive_PL123.txt"]
    assert acquired == [
        "https://www.youtube.com/watch?v=abcdefghijk",
        "https://www.youtube.com/watch?v=lmnopqrstuv",
    ]
    assert processed == [
        ([tmp_path / "downloads" / "abcdefghijk.flac"], []),
        ([tmp_path / "downloads" / "lmnopqrstuv.flac"], []),
    ]
    state = cache_mod.get_json("playlist_PL123")
    assert state is not None
    assert state["videos"]["abcdefghijk"]["status"] == "downloaded"


def test_run_workflow_skips_organized_playlist_entries(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    cache_mod.set_json(
        "playlist_PL123",
        {
            "playlist_id": "PL123",
            "videos": {"abcdefghijk": {"status": "organized"}},
        },
    )
    archives: list[Path] = []

    def fail_process(audio, split):
        raise AssertionError("organized playlist entries should be skipped")

    operations = service.WorkflowRunOperations(
        download_audio=lambda url, output, archive: False,
        process_audio=fail_process,
        acquire_soulseek=lambda request: [],
        prepopulate_archive=lambda archive: archives.append(archive),
        get_playlist_video_ids=lambda url: ["abcdefghijk"],
    )

    service.run_workflow(
        service.WorkflowRequest(
            raw="https://youtube.com/playlist?list=PL123",
            output=tmp_path / "downloads",
            splits=tmp_path / "splits",
        ),
        service.WorkflowOptions(audio_source="youtube"),
        operations=operations,
    )

    assert archives == [tmp_path / "cache" / "ytdlp_archive_PL123.txt"]
    state = cache_mod.get_json("playlist_PL123")
    assert state is not None
    assert state["videos"]["abcdefghijk"]["status"] == "organized"

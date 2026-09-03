"""Tests for the YouTube-first quality upgrade flow (Phase 5)."""

from pathlib import Path

import pytest

from muzik.core import cache as cache_mod
from muzik.core.quality import QualityDecision, QualityInfo, QualityPolicy
from muzik.core.sources.base import Candidate, CandidateFile, DownloadResult
from muzik.core.workflow import service
from muzik.core.workflow.cancellation import CancellationToken, WorkflowCancelled
from muzik.core.workflow.decisions import NonInteractiveWorkflowDecisions


class FakeSource:
    def __init__(self, *, candidates=None, download_result=None, download_error=None):
        self.candidates = candidates or []
        self.download_result = download_result
        self.download_error = download_error
        self.search_calls: list[object] = []
        self.download_calls: list[Candidate] = []

    def resolve(self, request):
        raise AssertionError("resolve is not used by the quality check")

    def search(self, resolved, *, prefer, limit):
        self.search_calls.append(resolved)
        return self.candidates

    def download(self, candidate, wait):
        self.download_calls.append(candidate)
        if self.download_error is not None:
            raise self.download_error
        return self.download_result


def _safe_candidate(
    *, duration: float, files: list[CandidateFile] | None = None
) -> Candidate:
    return Candidate(
        source="soulseek",
        source_id="peer:Song.flac",
        title="Song",
        user="peer",
        files=files or [CandidateFile(name="Song.flac", size=1, duration=duration)],
        score=100,
    )


def _lossy_quality() -> QualityInfo:
    return QualityInfo(format="mp3", lossless=False, bitrate=128, size=1)


def _lossless_quality() -> QualityInfo:
    return QualityInfo(format="flac", lossless=True, size=1)


@pytest.fixture
def youtube_file(tmp_path: Path, monkeypatch) -> Path:
    audio = tmp_path / "My Song [abcdefghijk].m4a"
    audio.write_bytes(b"audio")
    monkeypatch.setattr(
        service, "extract_metadata", lambda path: {"title": "Song", "artist": "Artist"}
    )
    monkeypatch.setattr(service, "get_duration", lambda path: 180.0)
    return audio


def test_off_policy_keeps_the_file_without_measuring(
    youtube_file: Path, monkeypatch
) -> None:
    def fail_measure(path):
        raise AssertionError("OFF policy must not measure the file")

    monkeypatch.setattr(service, "measure_quality", fail_measure)

    result = service.check_youtube_quality(
        [youtube_file],
        policy=QualityPolicy.OFF,
        min_bitrate=256,
        prefer="lossless",
        decisions=NonInteractiveWorkflowDecisions(),
    )

    assert result.audio_files == [youtube_file]
    assert result.decision == QualityDecision.KEEP
    assert result.replaced is False


def test_a_lossless_file_is_kept_without_searching(
    youtube_file: Path, monkeypatch
) -> None:
    monkeypatch.setattr(service, "measure_quality", lambda path: _lossless_quality())
    source = FakeSource()

    result = service.check_youtube_quality(
        [youtube_file],
        policy=QualityPolicy.AUTO,
        min_bitrate=256,
        prefer="lossless",
        decisions=NonInteractiveWorkflowDecisions(),
        source_factory=lambda: source,
    )

    assert result.audio_files == [youtube_file]
    assert result.decision == QualityDecision.KEEP
    assert source.search_calls == []


def test_auto_policy_replaces_with_a_safe_candidate(
    youtube_file: Path, tmp_path: Path, monkeypatch
) -> None:
    monkeypatch.setattr(service, "measure_quality", lambda path: _lossy_quality())
    replacement = tmp_path / "Song.flac"
    replacement.write_bytes(b"better audio")
    monkeypatch.setattr(
        service,
        "get_duration",
        lambda path: 180.0 if path == youtube_file else 181.0,
    )
    source = FakeSource(
        candidates=[_safe_candidate(duration=181.0)],
        download_result=DownloadResult(
            source="soulseek", source_id="x", files=[replacement], root=tmp_path
        ),
    )

    result = service.check_youtube_quality(
        [youtube_file],
        policy=QualityPolicy.AUTO,
        min_bitrate=256,
        prefer="lossless",
        decisions=NonInteractiveWorkflowDecisions(),
        source_factory=lambda: source,
    )

    assert result.audio_files == [replacement]
    assert result.replaced is True
    assert len(source.download_calls) == 1


def test_ask_policy_declines_without_confirmation(
    youtube_file: Path, tmp_path: Path, monkeypatch
) -> None:
    monkeypatch.setattr(service, "measure_quality", lambda path: _lossy_quality())
    source = FakeSource(candidates=[_safe_candidate(duration=180.0)])

    result = service.check_youtube_quality(
        [youtube_file],
        policy=QualityPolicy.ASK,
        min_bitrate=256,
        prefer="lossless",
        decisions=NonInteractiveWorkflowDecisions(confirm_quality_replacement=False),
        source_factory=lambda: source,
    )

    assert result.audio_files == [youtube_file]
    assert result.replaced is False
    assert source.download_calls == []


def test_ask_policy_replaces_once_confirmed(
    youtube_file: Path, tmp_path: Path, monkeypatch
) -> None:
    monkeypatch.setattr(service, "measure_quality", lambda path: _lossy_quality())
    replacement = tmp_path / "Song.flac"
    replacement.write_bytes(b"better audio")
    monkeypatch.setattr(
        service,
        "get_duration",
        lambda path: 180.0 if path == youtube_file else 180.0,
    )
    source = FakeSource(
        candidates=[_safe_candidate(duration=180.0)],
        download_result=DownloadResult(
            source="soulseek", source_id="x", files=[replacement], root=tmp_path
        ),
    )

    result = service.check_youtube_quality(
        [youtube_file],
        policy=QualityPolicy.ASK,
        min_bitrate=256,
        prefer="lossless",
        decisions=NonInteractiveWorkflowDecisions(confirm_quality_replacement=True),
        source_factory=lambda: source,
    )

    assert result.audio_files == [replacement]
    assert result.replaced is True


def test_no_safe_candidate_keeps_the_youtube_file(
    youtube_file: Path, monkeypatch
) -> None:
    monkeypatch.setattr(service, "measure_quality", lambda path: _lossy_quality())
    # Wrong artist/title text and a wildly different duration: filtered out
    # by candidate_matches_track before it ever reaches selection/download.
    unrelated = Candidate(
        source="soulseek",
        source_id="peer:Nope.flac",
        title="Nope",
        user="peer",
        files=[CandidateFile(name="Nope.flac", size=1, duration=9999.0)],
        score=50,
    )
    source = FakeSource(candidates=[unrelated])

    result = service.check_youtube_quality(
        [youtube_file],
        policy=QualityPolicy.AUTO,
        min_bitrate=256,
        prefer="lossless",
        decisions=NonInteractiveWorkflowDecisions(),
        source_factory=lambda: source,
    )

    assert result.audio_files == [youtube_file]
    assert result.decision == QualityDecision.KEEP_NO_SAFE_REPLACEMENT
    assert source.download_calls == []


def test_search_failure_keeps_the_youtube_file_and_does_not_raise(
    youtube_file: Path, monkeypatch
) -> None:
    monkeypatch.setattr(service, "measure_quality", lambda path: _lossy_quality())

    class FailingSource(FakeSource):
        def search(self, resolved, *, prefer, limit):
            raise RuntimeError("network down")

    result = service.check_youtube_quality(
        [youtube_file],
        policy=QualityPolicy.AUTO,
        min_bitrate=256,
        prefer="lossless",
        decisions=NonInteractiveWorkflowDecisions(),
        source_factory=FailingSource,
    )

    assert result.audio_files == [youtube_file]
    assert result.decision == QualityDecision.KEEP_NO_SAFE_REPLACEMENT


def test_download_failure_keeps_the_youtube_file(
    youtube_file: Path, monkeypatch
) -> None:
    monkeypatch.setattr(service, "measure_quality", lambda path: _lossy_quality())
    source = FakeSource(
        candidates=[_safe_candidate(duration=180.0)],
        download_error=RuntimeError("peer went offline"),
    )

    result = service.check_youtube_quality(
        [youtube_file],
        policy=QualityPolicy.AUTO,
        min_bitrate=256,
        prefer="lossless",
        decisions=NonInteractiveWorkflowDecisions(),
        source_factory=lambda: source,
    )

    assert result.audio_files == [youtube_file]
    assert result.decision == QualityDecision.KEEP_NO_SAFE_REPLACEMENT


def test_a_multi_file_replacement_becomes_a_pre_split_directory(
    youtube_file: Path, tmp_path: Path, monkeypatch
) -> None:
    monkeypatch.setattr(service, "measure_quality", lambda path: _lossy_quality())
    album_dir = tmp_path / "album"
    album_dir.mkdir()
    track_a = album_dir / "01.flac"
    track_b = album_dir / "02.flac"
    track_a.write_bytes(b"a")
    track_b.write_bytes(b"b")
    source = FakeSource(
        candidates=[
            _safe_candidate(
                duration=180.0,
                files=[
                    CandidateFile(name="01.flac", size=1, duration=90.0),
                    CandidateFile(name="02.flac", size=1, duration=90.0),
                ],
            )
        ],
        download_result=DownloadResult(
            source="soulseek",
            source_id="x",
            files=[track_a, track_b],
            root=album_dir,
        ),
    )

    result = service.check_youtube_quality(
        [youtube_file],
        policy=QualityPolicy.AUTO,
        min_bitrate=256,
        prefer="lossless",
        decisions=NonInteractiveWorkflowDecisions(),
        source_factory=lambda: source,
    )

    assert result.audio_files == []
    assert result.pre_split_dirs == [album_dir]
    assert result.replaced is True


def test_a_replacement_with_the_wrong_duration_is_rejected(
    youtube_file: Path, tmp_path: Path, monkeypatch
) -> None:
    monkeypatch.setattr(service, "measure_quality", lambda path: _lossy_quality())
    replacement = tmp_path / "Song.flac"
    replacement.write_bytes(b"different recording")
    monkeypatch.setattr(
        service,
        "get_duration",
        lambda path: 180.0 if path == youtube_file else 400.0,
    )
    source = FakeSource(
        candidates=[_safe_candidate(duration=180.0)],
        download_result=DownloadResult(
            source="soulseek", source_id="x", files=[replacement], root=tmp_path
        ),
    )

    result = service.check_youtube_quality(
        [youtube_file],
        policy=QualityPolicy.AUTO,
        min_bitrate=256,
        prefer="lossless",
        decisions=NonInteractiveWorkflowDecisions(),
        source_factory=lambda: source,
    )

    assert result.audio_files == [youtube_file]
    assert result.decision == QualityDecision.KEEP_NO_SAFE_REPLACEMENT


def test_a_single_file_replacement_keeps_the_youtube_chapter_sidecars(
    youtube_file: Path, tmp_path: Path, monkeypatch
) -> None:
    sidecar = youtube_file.with_name(youtube_file.stem + ".chapters.txt")
    sidecar.write_text("00:00 One\n00:10 Two\n", encoding="utf-8")
    monkeypatch.setattr(service, "measure_quality", lambda path: _lossy_quality())
    replacement = tmp_path / "Song.flac"
    replacement.write_bytes(b"better audio")
    monkeypatch.setattr(service, "get_duration", lambda path: 180.0)
    source = FakeSource(
        candidates=[_safe_candidate(duration=180.0)],
        download_result=DownloadResult(
            source="soulseek", source_id="x", files=[replacement], root=tmp_path
        ),
    )

    result = service.check_youtube_quality(
        [youtube_file],
        policy=QualityPolicy.AUTO,
        min_bitrate=256,
        prefer="lossless",
        decisions=NonInteractiveWorkflowDecisions(),
        source_factory=lambda: source,
    )

    replacement_sidecar = replacement.with_name(replacement.stem + ".chapters.txt")
    assert result.audio_files == [replacement]
    assert replacement_sidecar.exists()
    assert replacement_sidecar.read_text(encoding="utf-8") == sidecar.read_text(
        encoding="utf-8"
    )


def test_cancellation_stops_before_the_search(youtube_file: Path, monkeypatch) -> None:
    monkeypatch.setattr(service, "measure_quality", lambda path: _lossy_quality())
    token = CancellationToken()
    token.cancel()

    class UnreachableSource(FakeSource):
        def search(self, resolved, *, prefer, limit):
            raise AssertionError("search should not run once cancelled")

    with pytest.raises(WorkflowCancelled):
        service.check_youtube_quality(
            [youtube_file],
            policy=QualityPolicy.AUTO,
            min_bitrate=256,
            prefer="lossless",
            decisions=NonInteractiveWorkflowDecisions(),
            source_factory=UnreachableSource,
            cancellation=token,
        )


def test_auto_mode_youtube_url_no_longer_tries_soulseek_first(
    tmp_path: Path, monkeypatch
) -> None:
    """Regression guard: AUTO used to search Soulseek before YouTube for any
    input, including a YouTube URL. The source-routing fix in this phase
    stops that — YouTube input always downloads with YouTubeSource first."""
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    calls: list[str] = []

    def download(url: str, output: Path, archive: Path | None) -> bool:
        calls.append("youtube")
        output.mkdir(exist_ok=True)
        (output / "song [abcdefghijk].flac").write_bytes(b"audio")
        return True

    operations = service.WorkflowRunOperations(
        download_audio=download,
        process_audio=lambda *_: None,
        acquire_soulseek=lambda _: calls.append("soulseek") or [],
        prepopulate_archive=lambda _: None,
        get_playlist_video_ids=lambda _: [],
        soulseek_ready=lambda: True,
    )

    files, _ = service._acquire_single_workflow_inputs(
        service.WorkflowRequest(
            raw="https://youtube.com/watch?v=abcdefghijk",
            output=tmp_path / "out",
            splits=tmp_path / "splits",
        ),
        service.WorkflowOptions(audio_source="auto"),
        yt_id="abcdefghijk",
        operations=operations,
    )

    assert calls == ["youtube"]
    assert files == [tmp_path / "out" / "song [abcdefghijk].flac"]


def test_auto_mode_playlist_no_longer_tries_soulseek_per_video(
    tmp_path: Path, monkeypatch
) -> None:
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    calls: list[str] = []

    def download(url: str, output: Path, archive: Path | None) -> bool:
        calls.append("youtube")
        output.mkdir(parents=True, exist_ok=True)
        (output / "Song [abcdefghijk].flac").write_bytes(b"audio")
        return True

    operations = service.WorkflowRunOperations(
        download_audio=download,
        process_audio=lambda *_: None,
        acquire_soulseek=lambda _: calls.append("soulseek") or [],
        prepopulate_archive=lambda _: None,
        get_playlist_video_ids=lambda _: ["abcdefghijk"],
        soulseek_ready=lambda: True,
    )

    service.run_workflow(
        service.WorkflowRequest(
            raw="https://youtube.com/playlist?list=PL123",
            output=tmp_path / "downloads",
            splits=tmp_path / "splits",
        ),
        service.WorkflowOptions(audio_source="auto", no_organize=True),
        operations=operations,
    )

    assert calls == ["youtube"]


def test_quality_check_never_runs_before_the_youtube_download_completes(
    tmp_path: Path, monkeypatch
) -> None:
    """Ordering guard: the quality operation only ever sees files that
    already exist on disk, downloaded by YouTubeSource first."""
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    order: list[str] = []

    def download(url: str, output: Path, archive: Path | None) -> bool:
        order.append("youtube_download")
        output.mkdir(parents=True, exist_ok=True)
        (output / "song [abcdefghijk].flac").write_bytes(b"audio")
        return True

    def check_quality(audio_files: list[Path]) -> service.QualityUpgradeResult:
        order.append("quality_check")
        assert all(path.exists() for path in audio_files)
        return service.QualityUpgradeResult(
            audio_files=audio_files,
            pre_split_dirs=[],
            decision=QualityDecision.KEEP,
        )

    operations = service.WorkflowRunOperations(
        download_audio=download,
        process_audio=lambda *_: None,
        acquire_soulseek=lambda _: (_ for _ in ()).throw(
            AssertionError("Soulseek must not run for a YouTube URL")
        ),
        prepopulate_archive=lambda _: None,
        get_playlist_video_ids=lambda _: [],
        check_quality=check_quality,
    )

    service._acquire_single_workflow_inputs(
        service.WorkflowRequest(
            raw="https://youtube.com/watch?v=abcdefghijk",
            output=tmp_path / "out",
            splits=tmp_path / "splits",
        ),
        service.WorkflowOptions(audio_source="youtube"),
        yt_id="abcdefghijk",
        operations=operations,
    )

    assert order == ["youtube_download", "quality_check"]


def test_check_quality_operation_is_wired_into_single_workflow_input(
    tmp_path: Path, monkeypatch
) -> None:
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    replacement = tmp_path / "replacement.flac"
    replacement.write_bytes(b"upgraded")

    def download(url: str, output: Path, archive: Path | None) -> bool:
        output.mkdir(parents=True, exist_ok=True)
        (output / "song [abcdefghijk].flac").write_bytes(b"audio")
        return True

    operations = service.WorkflowRunOperations(
        download_audio=download,
        process_audio=lambda *_: None,
        acquire_soulseek=lambda _: [],
        prepopulate_archive=lambda _: None,
        get_playlist_video_ids=lambda _: [],
        check_quality=lambda audio_files: service.QualityUpgradeResult(
            audio_files=[replacement],
            pre_split_dirs=[],
            decision=QualityDecision.REPLACE,
            replaced=True,
        ),
    )

    files, pre_split_dirs = service._acquire_single_workflow_inputs(
        service.WorkflowRequest(
            raw="https://youtube.com/watch?v=abcdefghijk",
            output=tmp_path / "out",
            splits=tmp_path / "splits",
        ),
        service.WorkflowOptions(audio_source="youtube"),
        yt_id="abcdefghijk",
        operations=operations,
    )

    assert files == [replacement]
    assert pre_split_dirs == []

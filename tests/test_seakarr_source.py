from pathlib import Path

import pytest

from muzik.core import cache as cache_mod
from muzik.core.sources.base import Candidate, CandidateFile, QualityInfo
from muzik.core.sources.seakarr import (
    SeakarrSource,
    SoulseekError,
    candidate_from_result,
    remote_basename,
    remote_parent,
)
from muzik.core.workflow.cancellation import CancellationToken, WorkflowCancelled


class FakeJob:
    def __init__(self, *, status: str = "completed", value=None, error=None):
        self._status = status
        self._value = value
        self._error = error
        self.cancelled = False

    def poll(self) -> str:
        return self._status

    def cancel(self) -> None:
        self.cancelled = True
        self._status = "cancelled"

    def result(self):
        if self._error is not None:
            raise self._error
        return self._value


class FakeSession:
    def __init__(self, *, on_download=None):
        self.search_calls: list[tuple[str, float]] = []
        self.download_calls: list[tuple[str, str, int, str]] = []
        self.closed = False
        self.search_job: FakeJob | None = None
        self.download_job: FakeJob | None = None
        self._on_download = on_download

    def start_track_search(self, query: str, timeout_secs: float) -> FakeJob:
        self.search_calls.append((query, timeout_secs))
        assert self.search_job is not None, "test must set FakeSession.search_job"
        return self.search_job

    def start_download(
        self, username: str, filename: str, size: int, destination: str
    ) -> FakeJob:
        self.download_calls.append((username, filename, size, destination))
        if self._on_download is not None:
            return self._on_download(username, filename, size, destination)
        assert self.download_job is not None, "test must set FakeSession.download_job"
        return self.download_job

    def close(self) -> None:
        self.closed = True


def test_remote_basename_and_parent_split_on_backslashes() -> None:
    # Soulseek remote paths use Windows-style backslashes regardless of the
    # host platform; pathlib.Path.name would return the whole string on
    # POSIX for a backslash-only path, silently breaking file lookup.
    remote = "Music\\Artist\\Album\\01 One.flac"

    assert remote_basename(remote) == "01 One.flac"
    assert remote_parent(remote) == "Music/Artist/Album"


def test_remote_basename_handles_a_bare_filename() -> None:
    assert remote_basename("01 One.flac") == "01 One.flac"
    assert remote_parent("01 One.flac") == ""


def test_candidate_from_result_scores_lossless_result() -> None:
    result = {
        "username": "peer",
        "slots": 3,
        "speed": 500_000,
        "files": [
            {
                "name": "Artist\\Album\\01 One.flac",
                "size": 123_000_000,
                "bitrate_kbps": None,
                "duration_seconds": 180,
                "vbr": None,
                "sample_rate_hz": 44_100,
                "bit_depth": 16,
            },
            {
                "name": "Artist\\Album\\02 Two.flac",
                "size": 124_000_000,
                "bitrate_kbps": None,
                "duration_seconds": 190,
                "vbr": None,
                "sample_rate_hz": 44_100,
                "bit_depth": 16,
            },
        ],
    }

    candidate = candidate_from_result(
        result,
        query="Artist Album flac",
        expected_track_count=2,
    )

    assert candidate.source == "soulseek"
    assert candidate.user == "peer"
    assert candidate.title == "Album"
    assert candidate.quality.lossless is True
    assert candidate.files[0].quality.format == "flac"
    assert candidate.files[0].quality.sample_rate == 44_100
    assert candidate.files[0].quality.bit_depth == 16
    assert candidate.score > 100


def test_candidate_from_result_decodes_lossy_bitrate() -> None:
    result = {
        "username": "peer",
        "slots": 1,
        "speed": 100_000,
        "files": [
            {
                "name": "01 One.mp3",
                "size": 9_800_000,
                "bitrate_kbps": 320,
                "duration_seconds": 245,
                "vbr": False,
                "sample_rate_hz": 44_100,
                "bit_depth": None,
            }
        ],
    }

    candidate = candidate_from_result(result, query="Track")

    assert candidate.files[0].quality.bitrate == 320
    assert candidate.files[0].quality.lossless is False


def test_seakarr_source_requires_credentials() -> None:
    source = SeakarrSource(username="", password="secret")

    with pytest.raises(SoulseekError, match="username/password"):
        _ = source.session


def test_seakarr_source_search_returns_ranked_candidates() -> None:
    session = FakeSession()
    session.search_job = FakeJob(
        status="completed",
        value=[
            {
                "username": "peer",
                "slots": 1,
                "speed": 1,
                "files": [{"name": "01 One.flac", "size": 1}],
            }
        ],
    )
    source = SeakarrSource(username="u", password="p")
    source._session = session

    from muzik.core.sources.base import ResolvedTrack

    candidates = source.search(ResolvedTrack(title="Title", artist="Artist"))

    assert len(candidates) == 1
    assert candidates[0].user == "peer"
    assert session.search_calls == [("Artist Title flac", source.search_timeout)]


def test_seakarr_source_search_wraps_job_failure() -> None:
    session = FakeSession()
    session.search_job = FakeJob(status="failed", error=RuntimeError("peer offline"))
    source = SeakarrSource(username="u", password="p")
    source._session = session

    from muzik.core.sources.base import ResolvedTrack

    with pytest.raises(SoulseekError, match="Soulseek search failed"):
        source.search(ResolvedTrack(title="Title"))


def test_seakarr_source_download_writes_metadata_and_cache(
    tmp_path: Path, monkeypatch
) -> None:
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    audio = tmp_path / "01 One.flac"

    def fake_download(username, filename, size, destination):
        audio.write_bytes(b"a")
        return FakeJob(status="completed", value={"state": "completed"})

    session = FakeSession(on_download=fake_download)

    candidate = Candidate(
        source="soulseek",
        source_id="peer:01 One.flac",
        title="Album",
        user="peer",
        files=[
            CandidateFile(
                name="01 One.flac",
                size=1,
                quality=QualityInfo(format="flac", lossless=True),
            )
        ],
    )

    source = SeakarrSource(username="u", password="p", download_dir=tmp_path)
    source._session = session

    result = source.download(candidate, tmp_path, wait=True)

    assert result.files == [audio]
    assert result.metadata_path == tmp_path / "01 One.muzik.json"
    assert result.metadata_path.exists()
    cache_key = cache_mod.download_cache_key("soulseek", candidate.source_id)
    assert cache_mod.get(cache_key) == str(audio.resolve())


def test_seakarr_source_download_reports_failure(tmp_path: Path) -> None:
    session = FakeSession()
    session.download_job = FakeJob(
        status="failed", value={"state": "failed", "reason": "peer went offline"}
    )
    candidate = Candidate(
        source="soulseek",
        source_id="peer:01 One.flac",
        title="Album",
        user="peer",
        files=[CandidateFile(name="01 One.flac", size=1)],
    )
    source = SeakarrSource(username="u", password="p", download_dir=tmp_path)
    source._session = session

    with pytest.raises(SoulseekError, match="peer went offline"):
        source.download(candidate, tmp_path, wait=True)


def test_seakarr_source_download_stops_immediately_when_already_cancelled(
    tmp_path: Path,
) -> None:
    session = FakeSession()
    session.download_job = FakeJob(status="running")
    candidate = Candidate(
        source="soulseek",
        source_id="peer:01 One.flac",
        title="Album",
        user="peer",
        files=[CandidateFile(name="01 One.flac", size=1)],
    )
    source = SeakarrSource(username="u", password="p", download_dir=tmp_path)
    source._session = session
    token = CancellationToken()
    token.cancel()

    with pytest.raises(WorkflowCancelled):
        source.download(candidate, tmp_path, wait=True, cancellation=token)


def test_seakarr_source_check_never_exposes_the_password(monkeypatch) -> None:
    # Force the connection-failure path deterministically: a real
    # SeakarrSession.connect() would reach the live Soulseek network, which
    # is unsuitable for a unit test (slow, flaky, and Soulseek auto-registers
    # unknown usernames, so a "fake" login can silently succeed).
    import muzik.core.sources.seakarr as seakarr_mod

    def failing_loader():
        raise SoulseekError("Seakarr connection failed: simulated failure")

    monkeypatch.setattr(seakarr_mod, "_load_seakarr_module", failing_loader)
    source = SeakarrSource(username="listener", password="super-secret")

    info = source.check()

    assert "super-secret" not in str(info)
    assert info["connected"] is False

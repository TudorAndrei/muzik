from pathlib import Path

from muzik.commands import soulseek
from muzik.core import cache as cache_mod
from muzik.core.sources.base import (
    Candidate,
    CandidateFile,
    DownloadResult,
    QualityInfo,
    ResolvedRelease,
)


def _candidate() -> Candidate:
    return Candidate(
        source="soulseek",
        source_id="peer:/Music/Artist/Album",
        title="Album",
        user="peer",
        path="/Music/Artist/Album",
        files=[
            CandidateFile(
                name="01 One.flac",
                size=10,
                quality=QualityInfo(format="flac", lossless=True),
            )
        ],
        quality=QualityInfo(format="flac", lossless=True),
        score=120,
    )


def test_soulseek_check_command_uses_source(monkeypatch) -> None:
    calls = {"check": 0}

    class FakeSource:
        def check(self):
            calls["check"] += 1
            return {
                "url": "http://localhost:5030",
                "download_dir": "/tmp/slskd",
                "auth_valid": True,
                "server_state": "Connected",
                "server_connected": True,
                "server_logged_in": True,
            }

    monkeypatch.setattr(soulseek, "_source", FakeSource)

    soulseek.check_cmd()

    assert calls == {"check": 1}


def test_soulseek_search_command_resolves_and_searches(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    calls: list[str] = []

    class FakeSource:
        def resolve(self, request):
            calls.append(f"resolve:{request.raw}:{request.prefer_format}")
            return ResolvedRelease(title="Album", artist="Artist")

        def search(self, resolved, *, prefer, limit):
            calls.append(f"search:{resolved.title}:{prefer}:{limit}")
            return [_candidate()]

    monkeypatch.setattr(soulseek, "_source", FakeSource)

    soulseek.search_cmd("Artist - Album", prefer="flac", limit=5)

    assert calls == ["resolve:Artist - Album:flac", "search:Album:flac:5"]


def test_soulseek_download_command_downloads_top_candidate(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    calls: list[str] = []
    audio = tmp_path / "01 One.flac"

    class FakeSource:
        def resolve(self, request):
            calls.append(f"resolve:{request.raw}")
            return ResolvedRelease(title="Album", artist="Artist")

        def search(self, resolved, *, prefer, limit):
            calls.append(f"search:{prefer}:{limit}")
            return [_candidate()]

        def download(self, candidate, output, *, wait):
            calls.append(f"download:{candidate.source_id}:{output}:{wait}")
            return DownloadResult(
                source="soulseek",
                source_id=candidate.source_id,
                files=[audio],
                root=tmp_path,
                metadata_path=tmp_path / ".muzik.json",
            )

    def fail_organize_cmd(**kwargs):
        raise AssertionError("--no-organize should skip beets")

    monkeypatch.setattr(soulseek, "_source", FakeSource)
    monkeypatch.setattr(soulseek, "organize_cmd", fail_organize_cmd)

    soulseek.download_cmd(
        query="Artist - Album",
        prefer="flac",
        limit=3,
        output=tmp_path,
        no_interactive=True,
        no_wait=True,
        no_organize=True,
        import_=False,
        tag_only=False,
        dry_run=False,
        candidate_id=None,
    )

    assert calls == [
        "resolve:Artist - Album",
        "search:flac:3",
        f"download:peer:/Music/Artist/Album:{tmp_path}:False",
    ]


def test_soulseek_download_command_uses_cached_candidate(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    candidate = _candidate()
    candidate_id = soulseek._candidate_id(candidate)
    soulseek._store_candidates([candidate])
    calls: list[str] = []

    class FakeSource:
        def download(self, candidate, output, *, wait):
            calls.append(f"download:{candidate.source_id}:{output}:{wait}")
            return DownloadResult(
                source="soulseek",
                source_id=candidate.source_id,
                files=[],
                root=tmp_path,
            )

    monkeypatch.setattr(soulseek, "_source", FakeSource)

    soulseek.download_cmd(
        query=None,
        prefer="flac",
        limit=3,
        output=tmp_path,
        no_interactive=True,
        no_wait=False,
        no_organize=True,
        import_=False,
        tag_only=False,
        dry_run=False,
        candidate_id=candidate_id,
    )

    assert calls == [f"download:peer:/Music/Artist/Album:{tmp_path}:True"]

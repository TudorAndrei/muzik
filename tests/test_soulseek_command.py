from pathlib import Path

from types import SimpleNamespace

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
                "username": "muziklistener",
                "server": "server.slsknet.org:2416",
                "download_dir": "/tmp/soulseek",
                "connected": True,
                "detail": "Connected",
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


class FakeLibrary:
    def __init__(self, music: Path) -> None:
        self.directory = str(music)
        self._items: list[SimpleNamespace] = []

    def items(self, query=None):
        return list(self._items)


def _library(tmp_path: Path) -> tuple[FakeLibrary, Path]:
    music = tmp_path / "music"
    music.mkdir()
    return FakeLibrary(music), music


def _add_item(
    lib: FakeLibrary,
    path: Path,
    *,
    title: str,
    artist: str,
    album: str,
    length: float = 200.0,
) -> None:
    path.write_bytes(b"x")
    lib._items.append(
        SimpleNamespace(
            path=str(path).encode(),
            title=title,
            artist=artist,
            album=album,
            albumartist=artist,
            length=length,
        )
    )


def _lossy_quality(bitrate: int = 128) -> QualityInfo:
    return QualityInfo(format="mp3", lossless=False, bitrate=bitrate)


def _better_candidate(*, title: str, artist: str, duration: float) -> Candidate:
    return Candidate(
        source="soulseek",
        source_id=f"peer:/Music/{artist}/{title}",
        title=f"{artist} - {title}",
        user="peer",
        path=f"/Music/{artist}/{title}.flac",
        files=[
            CandidateFile(
                name=f"{title}.flac",
                size=10,
                duration=duration,
                quality=QualityInfo(format="flac", lossless=True),
            )
        ],
        quality=QualityInfo(format="flac", lossless=True),
        score=100,
    )


def test_check_library_skips_tracks_already_at_or_above_the_threshold(
    tmp_path: Path,
    monkeypatch,
) -> None:
    lib, music = _library(tmp_path)
    _add_item(lib, music / "good.flac", title="Good", artist="Artist", album="Album")
    monkeypatch.setattr(soulseek, "open_library_for_reads", lambda config: lib)
    monkeypatch.setattr(
        soulseek,
        "measure_quality",
        lambda path: QualityInfo(format="flac", lossless=True),
    )

    def fail_search(*args, **kwargs):
        raise AssertionError("a fine track must not be searched")

    monkeypatch.setattr(
        soulseek, "_source", lambda: type("S", (), {"search": fail_search})()
    )

    soulseek.check_library_cmd(
        query=None, min_bitrate=256, prefer="lossless", limit=20, config=None
    )


def test_check_library_reports_a_better_replacement(
    tmp_path: Path,
    monkeypatch,
    capsys,
) -> None:
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    lib, music = _library(tmp_path)
    _add_item(
        lib,
        music / "low.mp3",
        title="Low Quality",
        artist="Test Artist",
        album="Album",
        length=200.0,
    )
    monkeypatch.setattr(soulseek, "open_library_for_reads", lambda config: lib)
    monkeypatch.setattr(soulseek, "measure_quality", lambda path: _lossy_quality(128))
    candidate = _better_candidate(
        title="Low Quality", artist="Test Artist", duration=200.0
    )

    class FakeSource:
        def search(self, track, *, prefer, limit):
            return [candidate]

    monkeypatch.setattr(soulseek, "_source", lambda: FakeSource())

    soulseek.check_library_cmd(
        query=None, min_bitrate=256, prefer="lossless", limit=20, config=None
    )

    captured = capsys.readouterr()
    assert "replacement found" in captured.err
    assert "Test Artist" in captured.err
    assert "1 replacement(s) found" in captured.err
    stored = soulseek._load_candidate(soulseek._candidate_id(candidate))
    assert stored.source_id == candidate.source_id


def test_check_library_reports_no_safe_match_for_a_wrong_recording(
    tmp_path: Path,
    monkeypatch,
    capsys,
) -> None:
    lib, music = _library(tmp_path)
    _add_item(
        lib,
        music / "low.mp3",
        title="Low Quality",
        artist="Test Artist",
        album="Album",
        length=200.0,
    )
    monkeypatch.setattr(soulseek, "open_library_for_reads", lambda config: lib)
    monkeypatch.setattr(soulseek, "measure_quality", lambda path: _lossy_quality(128))
    # Duration is far off and the text is unrelated — candidate_matches_track
    # must reject this, so it must not be reported as a replacement.
    wrong = _better_candidate(title="Unrelated", artist="Nobody", duration=9999.0)

    class FakeSource:
        def search(self, track, *, prefer, limit):
            return [wrong]

    monkeypatch.setattr(soulseek, "_source", lambda: FakeSource())

    soulseek.check_library_cmd(
        query=None, min_bitrate=256, prefer="lossless", limit=20, config=None
    )

    captured = capsys.readouterr()
    assert "no safe match" in captured.err
    assert "0 replacement(s) found" in captured.err


def test_check_library_reports_a_failed_search_and_keeps_scanning(
    tmp_path: Path,
    monkeypatch,
    capsys,
) -> None:
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    lib, music = _library(tmp_path)
    _add_item(
        lib,
        music / "one.mp3",
        title="One",
        artist="Artist One",
        album="Album",
        length=200.0,
    )
    _add_item(
        lib,
        music / "two.mp3",
        title="Two",
        artist="Artist Two",
        album="Album",
        length=200.0,
    )
    monkeypatch.setattr(soulseek, "open_library_for_reads", lambda config: lib)
    monkeypatch.setattr(soulseek, "measure_quality", lambda path: _lossy_quality(128))
    good = _better_candidate(title="Two", artist="Artist Two", duration=200.0)

    class FakeSource:
        def search(self, track, *, prefer, limit):
            if track.artist == "Artist One":
                raise RuntimeError("Soulseek timed out")
            return [good]

    monkeypatch.setattr(soulseek, "_source", lambda: FakeSource())

    soulseek.check_library_cmd(
        query=None, min_bitrate=256, prefer="lossless", limit=20, config=None
    )

    captured = capsys.readouterr()
    assert "search failed" in captured.err
    assert "replacement found" in captured.err
    assert "Scanned 2 track(s); 2 below 256kbps; 1 replacement(s) found." in (
        captured.err
    )


def test_check_library_limit_caps_searches_and_reports_the_remainder(
    tmp_path: Path,
    monkeypatch,
    capsys,
) -> None:
    lib, music = _library(tmp_path)
    for name in ("one", "two", "three"):
        _add_item(
            lib,
            music / f"{name}.mp3",
            title=name,
            artist=name,
            album="Album",
            length=200.0,
        )
    monkeypatch.setattr(soulseek, "open_library_for_reads", lambda config: lib)
    monkeypatch.setattr(soulseek, "measure_quality", lambda path: _lossy_quality(128))
    calls: list[str] = []

    class FakeSource:
        def search(self, track, *, prefer, limit):
            calls.append(track.artist)
            return []

    monkeypatch.setattr(soulseek, "_source", lambda: FakeSource())

    soulseek.check_library_cmd(
        query=None, min_bitrate=256, prefer="lossless", limit=1, config=None
    )

    assert len(calls) == 1
    captured = capsys.readouterr()
    assert "2 more below-threshold track(s) not searched" in captured.err

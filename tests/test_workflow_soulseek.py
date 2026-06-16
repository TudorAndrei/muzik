from pathlib import Path

import pytest
import typer

from muzik.commands import workflow
from muzik.core import cache as cache_mod
from muzik.core.sources.base import Candidate, DownloadResult, ResolvedRelease


def test_workflow_plain_text_query_uses_soulseek_source(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    audio = tmp_path / "downloads" / "01 One.flac"
    audio.parent.mkdir()
    audio.write_bytes(b"")

    calls = {"resolve": 0, "search": 0, "download": 0}

    class FakeSoulseekSource:
        def resolve(self, request):
            calls["resolve"] += 1
            assert request.raw == "Artist - Album"
            return ResolvedRelease(
                title="Album",
                artist="Artist",
                album="Album",
                source="soulseek",
                source_id=request.raw,
            )

        def search(self, resolved, *, prefer, limit):
            calls["search"] += 1
            assert prefer == "flac"
            assert limit == 10
            return [
                Candidate(
                    source="soulseek",
                    source_id="peer:/Artist/Album",
                    title=resolved.title,
                    user="peer",
                    score=100,
                )
            ]

        def download(self, candidate, wait):
            calls["download"] += 1
            assert wait is True
            return DownloadResult(
                source="soulseek",
                source_id=candidate.source_id,
                files=[audio],
                root=audio.parent,
                metadata_path=audio.with_suffix(".muzik.json"),
            )

    monkeypatch.setattr(workflow, "SoulseekSource", FakeSoulseekSource)
    monkeypatch.setattr(workflow, "find_chapters", lambda path: [])
    monkeypatch.setattr(workflow, "get_duration", lambda path: 180)

    workflow.workflow_cmd(
        url="Artist - Album",
        output=tmp_path / "downloads",
        splits=tmp_path / "splits",
        review=False,
        no_split=False,
        no_organize=True,
        import_=False,
        tag_only=False,
        dry_run=False,
        jobs=0,
        config=None,
        keep_source=False,
        force=False,
        metadata_source="auto",
        audio_source="soulseek",
        prefer="flac",
        fallback="none",
        interactive=False,
    )

    assert calls == {"resolve": 1, "search": 1, "download": 1}
    state = cache_mod.get_json(
        cache_mod.workflow_cache_key("soulseek", "Artist - Album")
    )
    assert state is not None
    assert state["status"] == "downloaded"
    assert state["source_id"] == "peer:/Artist/Album"


def test_workflow_youtube_url_uses_youtube_metadata_for_soulseek_search(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    audio = tmp_path / "downloads" / "Song.flac"
    audio.parent.mkdir()
    audio.write_bytes(b"")

    calls = {"youtube_resolve": 0, "soulseek_search": 0}

    class FakeYouTubeSource:
        def resolve(self, request):
            calls["youtube_resolve"] += 1
            assert request.raw == "https://youtube.com/watch?v=abcdefghijk"
            return ResolvedRelease(
                title="Album",
                artist="Artist",
                album="Album",
                source="youtube",
                source_id="abcdefghijk",
            )

    class FakeSoulseekSource:
        def search(self, resolved, *, prefer, limit):
            calls["soulseek_search"] += 1
            assert resolved.source == "youtube"
            assert resolved.artist == "Artist"
            return [
                Candidate(
                    source="soulseek",
                    source_id="peer:/Artist/Album",
                    title="Album",
                    user="peer",
                    score=100,
                )
            ]

        def download(self, candidate, wait):
            return DownloadResult(
                source="soulseek",
                source_id=candidate.source_id,
                files=[audio],
                root=audio.parent,
            )

    def fail_download_cmd(**kwargs):
        raise AssertionError(
            "YouTube audio should not be downloaded on Soulseek success"
        )

    monkeypatch.setattr(workflow, "YouTubeSource", FakeYouTubeSource)
    monkeypatch.setattr(workflow, "SoulseekSource", FakeSoulseekSource)
    monkeypatch.setattr(workflow, "download_cmd", fail_download_cmd)
    monkeypatch.setattr(workflow, "find_chapters", lambda path: [])
    monkeypatch.setattr(workflow, "get_duration", lambda path: 180)

    workflow.workflow_cmd(
        url="https://youtube.com/watch?v=abcdefghijk",
        output=tmp_path / "downloads",
        splits=tmp_path / "splits",
        review=False,
        no_split=False,
        no_organize=True,
        import_=False,
        tag_only=False,
        dry_run=False,
        jobs=0,
        config=None,
        keep_source=False,
        force=False,
        metadata_source="youtube",
        audio_source="soulseek",
        prefer="flac",
        fallback="none",
        interactive=False,
    )

    assert calls == {"youtube_resolve": 1, "soulseek_search": 1}


def test_workflow_falls_back_to_youtube_when_soulseek_has_no_candidates(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    calls = {"youtube_download": 0}

    class FakeYouTubeSource:
        def resolve(self, request):
            return ResolvedRelease(
                title="Album",
                artist="Artist",
                album="Album",
                source="youtube",
                source_id="abcdefghijk",
            )

    class FakeSoulseekSource:
        def search(self, resolved, *, prefer, limit):
            return []

    def fake_download_cmd(**kwargs):
        calls["youtube_download"] += 1
        output = kwargs["output"]
        output.mkdir(parents=True, exist_ok=True)
        (output / "Fallback [abcdefghijk].flac").write_bytes(b"")

    monkeypatch.setattr(workflow, "YouTubeSource", FakeYouTubeSource)
    monkeypatch.setattr(workflow, "SoulseekSource", FakeSoulseekSource)
    monkeypatch.setattr(workflow, "download_cmd", fake_download_cmd)
    monkeypatch.setattr(workflow, "find_chapters", lambda path: [])
    monkeypatch.setattr(workflow, "get_duration", lambda path: 180)

    workflow.workflow_cmd(
        url="https://youtube.com/watch?v=abcdefghijk",
        output=tmp_path / "downloads",
        splits=tmp_path / "splits",
        review=False,
        no_split=False,
        no_organize=True,
        import_=False,
        tag_only=False,
        dry_run=False,
        jobs=0,
        config=None,
        keep_source=False,
        force=False,
        metadata_source="youtube",
        audio_source="soulseek",
        prefer="flac",
        fallback="youtube",
        interactive=False,
    )

    assert calls["youtube_download"] == 1


def test_workflow_reads_legacy_youtube_cache(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    cached = tmp_path / "downloads" / "Cached [abcdefghijk].flac"
    cached.parent.mkdir()
    cached.write_bytes(b"")
    cache_mod.set("yt_abcdefghijk", str(cached))

    def fail_download(**kwargs):
        raise AssertionError("workflow should not download when legacy cache is valid")

    monkeypatch.setattr(workflow, "download_cmd", fail_download)
    monkeypatch.setattr(workflow, "find_chapters", lambda path: [])
    monkeypatch.setattr(workflow, "get_duration", lambda path: None)

    workflow.workflow_cmd(
        url="https://youtube.com/watch?v=abcdefghijk",
        output=tmp_path / "downloads",
        splits=tmp_path / "splits",
        review=False,
        no_split=False,
        no_organize=True,
        import_=False,
        tag_only=False,
        dry_run=False,
        jobs=0,
        config=None,
        keep_source=False,
        force=False,
        metadata_source="auto",
        audio_source="youtube",
        prefer="lossless",
        fallback="youtube",
        interactive=False,
    )


def test_workflow_youtube_audio_source_uses_download_cmd(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    calls: list[str] = []

    def fake_download_cmd(**kwargs):
        calls.append(kwargs["url"])
        output = kwargs["output"]
        output.mkdir(parents=True, exist_ok=True)
        (output / "Downloaded [abcdefghijk].flac").write_bytes(b"audio")

    monkeypatch.setattr(workflow, "download_cmd", fake_download_cmd)
    monkeypatch.setattr(workflow, "find_chapters", lambda path: [])
    monkeypatch.setattr(workflow, "get_duration", lambda path: 180)

    workflow.workflow_cmd(
        url="https://youtube.com/watch?v=abcdefghijk",
        output=tmp_path / "downloads",
        splits=tmp_path / "splits",
        review=False,
        no_split=False,
        no_organize=True,
        import_=False,
        tag_only=False,
        dry_run=False,
        jobs=0,
        config=None,
        keep_source=False,
        force=False,
        metadata_source="auto",
        audio_source="youtube",
        prefer="lossless",
        fallback="youtube",
        interactive=False,
    )

    assert calls == ["https://youtube.com/watch?v=abcdefghijk"]
    assert cache_mod.get("yt_abcdefghijk") == str(
        (tmp_path / "downloads" / "Downloaded [abcdefghijk].flac").resolve()
    )


def test_workflow_local_folder_organizes_directory_once(
    tmp_path: Path,
    monkeypatch,
) -> None:
    album = tmp_path / "Album"
    album.mkdir()
    (album / "cover.jpg").write_bytes(b"")
    (album / "01 One.flac").write_bytes(b"")
    (album / "02 Two.flac").write_bytes(b"")
    organized: list[Path] = []

    monkeypatch.setattr(workflow, "find_chapters", lambda path: [])
    monkeypatch.setattr(workflow, "get_duration", lambda path: 180)

    def fake_organize_cmd(**kwargs):
        organized.append(kwargs["directory"])

    monkeypatch.setattr(workflow, "organize_cmd", fake_organize_cmd)

    workflow.workflow_cmd(
        url=str(album),
        output=tmp_path / "downloads",
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
        metadata_source="auto",
        audio_source="soulseek",
        prefer="flac",
        fallback="none",
        interactive=False,
    )

    assert organized == [album]


def test_process_audio_files_rejects_unprobeable_audio_before_organize(
    tmp_path: Path,
    monkeypatch,
) -> None:
    bad = tmp_path / "bad.flac"
    bad.write_bytes(b"")

    monkeypatch.setattr(workflow, "get_duration", lambda path: None)

    def fail_organize_cmd(**kwargs):
        raise AssertionError("invalid audio should not be organized")

    monkeypatch.setattr(workflow, "organize_cmd", fail_organize_cmd)

    with pytest.raises(typer.Exit) as exc:
        workflow._process_audio_files(
            audio_inputs=[bad],
            pre_split_dirs=[],
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
        )

    assert exc.value.exit_code == 1


def test_workflow_playlist_uses_soulseek_per_video(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    video_ids = ["abcdefghijk", "lmnopqrstuv"]
    calls: list[str] = []

    monkeypatch.setattr(workflow, "_get_playlist_video_ids", lambda url: video_ids)
    monkeypatch.setattr(workflow, "_prepopulate_archive", lambda archive: None)
    monkeypatch.setattr(workflow, "find_chapters", lambda path: [])
    monkeypatch.setattr(workflow, "get_duration", lambda path: None)

    def fake_acquire(request, *, prefer, interactive, fallback, decisions, events):
        calls.append(request)
        audio = tmp_path / "downloads" / f"{request[-11:]}.flac"
        audio.parent.mkdir(exist_ok=True)
        audio.write_bytes(b"")
        return [audio]

    monkeypatch.setattr(workflow, "_acquire_from_soulseek", fake_acquire)

    workflow.workflow_cmd(
        url="https://youtube.com/playlist?list=PL123",
        output=tmp_path / "downloads",
        splits=tmp_path / "splits",
        review=False,
        no_split=False,
        no_organize=True,
        import_=False,
        tag_only=False,
        dry_run=False,
        jobs=0,
        config=None,
        keep_source=False,
        force=False,
        metadata_source="youtube",
        audio_source="soulseek",
        prefer="flac",
        fallback="none",
        interactive=False,
    )

    assert calls == [
        "https://www.youtube.com/watch?v=abcdefghijk",
        "https://www.youtube.com/watch?v=lmnopqrstuv",
    ]
    state = cache_mod.get_json("playlist_PL123")
    assert state is not None
    assert state["videos"]["abcdefghijk"]["source"] == "soulseek"
    assert state["videos"]["lmnopqrstuv"]["status"] == "downloaded"

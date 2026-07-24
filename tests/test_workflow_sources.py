from pathlib import Path

import pytest

from muzik.commands import workflow
from muzik.core import cache as cache_mod
from muzik.core.chapters import Chapter
from muzik.core.workflow import service


@pytest.mark.parametrize(
    "factory",
    [
        lambda: service.WorkflowOptions(metadata_source="invalid"),
        lambda: service.WorkflowOptions(audio_source="invalid"),
        lambda: service.WorkflowOptions(fallback="invalid"),
    ],
)
def test_workflow_options_reject_invalid_source_policies(factory) -> None:
    with pytest.raises(ValueError):
        factory()


def test_local_input_bypasses_remote_source_routing(tmp_path: Path) -> None:
    audio = tmp_path / "local.flac"
    audio.write_bytes(b"audio")
    calls: list[str] = []
    operations = service.WorkflowRunOperations(
        download_audio=lambda *_: calls.append("youtube") or True,
        process_audio=lambda *_: None,
        acquire_soulseek=lambda _: calls.append("soulseek") or [],
        prepopulate_archive=lambda _: None,
        get_playlist_video_ids=lambda _: [],
        soulseek_ready=lambda: True,
    )

    files, _ = service._acquire_single_workflow_inputs(
        service.WorkflowRequest(
            raw=str(audio), output=tmp_path / "out", splits=tmp_path / "splits"
        ),
        service.WorkflowOptions(audio_source=service.AudioSource.AUTO),
        yt_id=None,
        operations=operations,
    )

    assert files == [audio]
    assert calls == []


def test_auto_audio_uses_ready_soulseek_before_youtube(tmp_path: Path) -> None:
    audio = tmp_path / "soulseek.flac"
    calls: list[str] = []
    operations = service.WorkflowRunOperations(
        download_audio=lambda *_: calls.append("youtube") or True,
        process_audio=lambda *_: None,
        acquire_soulseek=lambda _: calls.append("soulseek") or [audio],
        prepopulate_archive=lambda _: None,
        get_playlist_video_ids=lambda _: [],
        soulseek_ready=lambda: True,
    )

    files, _ = service._acquire_single_workflow_inputs(
        service.WorkflowRequest(
            raw="Artist - Album", output=tmp_path / "out", splits=tmp_path / "splits"
        ),
        service.WorkflowOptions(audio_source="auto", fallback="none"),
        yt_id=None,
        operations=operations,
    )

    assert files == [audio]
    assert calls == ["soulseek"]


def test_auto_audio_uses_youtube_when_soulseek_is_not_ready(
    tmp_path: Path, monkeypatch
) -> None:
    output = tmp_path / "out"
    calls: list[str] = []
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")

    def download(url: str, destination: Path, archive: Path | None) -> bool:
        calls.append("youtube")
        destination.mkdir()
        (destination / "song [abcdefghijk].flac").write_bytes(b"audio")
        return True

    operations = service.WorkflowRunOperations(
        download_audio=download,
        process_audio=lambda *_: None,
        acquire_soulseek=lambda _: calls.append("soulseek") or [],
        prepopulate_archive=lambda _: None,
        get_playlist_video_ids=lambda _: [],
        soulseek_ready=lambda: False,
    )

    files, _ = service._acquire_single_workflow_inputs(
        service.WorkflowRequest(
            raw="https://youtube.com/watch?v=abcdefghijk",
            output=output,
            splits=tmp_path / "splits",
        ),
        service.WorkflowOptions(audio_source="auto"),
        yt_id="abcdefghijk",
        operations=operations,
    )

    assert files == [output / "song [abcdefghijk].flac"]
    assert calls == ["youtube"]


def test_dry_run_does_not_acquire_audio(tmp_path: Path) -> None:
    operations = service.WorkflowRunOperations(
        download_audio=lambda *_: pytest.fail("download should not run"),
        process_audio=lambda *_: None,
        acquire_soulseek=lambda _: pytest.fail("Soulseek should not run"),
        prepopulate_archive=lambda _: None,
        get_playlist_video_ids=lambda _: [],
        soulseek_ready=lambda: pytest.fail("readiness should not run"),
    )

    files, split_dirs = service._acquire_single_workflow_inputs(
        service.WorkflowRequest(
            raw="Artist - Album", output=tmp_path / "out", splits=tmp_path / "splits"
        ),
        service.WorkflowOptions(dry_run=True, audio_source="auto"),
        yt_id=None,
        operations=operations,
    )

    assert files == []
    assert split_dirs == []


def test_metadata_none_keeps_local_chapters_without_network(
    tmp_path: Path, monkeypatch
) -> None:
    audio = tmp_path / "album.flac"
    local_chapters = [Chapter(index=1, start=0, end=None, title="One")]
    monkeypatch.setattr(workflow, "find_chapters", lambda _: local_chapters)
    monkeypatch.setattr(
        workflow,
        "lookup_chapters",
        lambda *_: pytest.fail("MusicBrainz should not run"),
    )

    assert (
        workflow._get_chapters_for(
            audio, False, metadata_source=service.MetadataSource.NONE
        )
        == local_chapters
    )


@pytest.mark.parametrize(
    "metadata_source", [service.MetadataSource.NONE, service.MetadataSource.YOUTUBE]
)
def test_metadata_none_and_youtube_skip_musicbrainz(
    tmp_path: Path, monkeypatch, metadata_source: service.MetadataSource
) -> None:
    audio = tmp_path / "album.flac"
    monkeypatch.setattr(workflow, "find_chapters", lambda _: [])
    monkeypatch.setattr(workflow, "get_duration", lambda _: 3600)
    monkeypatch.delenv("OPENROUTER_API_KEY", raising=False)
    monkeypatch.setattr(
        workflow,
        "lookup_chapters",
        lambda *_: pytest.fail("MusicBrainz should not run"),
    )

    assert (
        workflow._get_chapters_for(audio, False, metadata_source=metadata_source)
        is None
    )


@pytest.mark.parametrize(
    "metadata_source", [service.MetadataSource.MUSICBRAINZ, service.MetadataSource.AUTO]
)
def test_metadata_musicbrainz_and_auto_query_musicbrainz(
    tmp_path: Path, monkeypatch, metadata_source: service.MetadataSource
) -> None:
    audio = tmp_path / "album.flac"
    calls: list[str] = []
    monkeypatch.setattr(workflow, "find_chapters", lambda _: [])
    monkeypatch.setattr(workflow, "get_duration", lambda _: 3600)
    monkeypatch.setattr(
        workflow,
        "extract_metadata",
        lambda _: {"artist": "Artist", "album": "Album", "year": "2026"},
    )
    monkeypatch.delenv("OPENROUTER_API_KEY", raising=False)
    monkeypatch.setattr(
        workflow,
        "lookup_chapters",
        lambda *args: calls.append("musicbrainz") or ([], "", "none"),
    )

    assert (
        workflow._get_chapters_for(audio, False, metadata_source=metadata_source)
        is None
    )
    assert calls == ["musicbrainz"]

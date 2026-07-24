from pathlib import Path

from muzik.commands import workflow
from muzik.core import cache as cache_mod


def test_cli_routes_spotify_export_to_soulseek_without_media_download(
    tmp_path: Path,
    monkeypatch,
) -> None:
    """The CLI accepts an export file and only uses its metadata as a query."""
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    export = Path("tests/fixtures/spotify/playlist_v1.json").resolve()
    audio = tmp_path / "downloads" / "fixture.flac"
    audio.parent.mkdir()
    audio.write_bytes(b"audio")
    queries: list[str] = []
    processed: list[list[Path]] = []

    def acquire(query: str, **_kwargs: object) -> list[Path]:
        queries.append(query)
        return [audio]

    monkeypatch.setattr(workflow, "_acquire_from_soulseek", acquire)
    monkeypatch.setattr(
        workflow,
        "_process_audio_files",
        lambda *, audio_inputs, **_kwargs: processed.append(audio_inputs),
    )
    monkeypatch.setattr(
        workflow,
        "_download_audio",
        lambda **_kwargs: (_ for _ in ()).throw(
            AssertionError("Spotify media must never reach yt-dlp")
        ),
    )

    workflow.workflow_cmd(
        url=str(export),
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
        prefer="lossless",
        fallback="none",
        interactive=False,
    )

    assert queries == ["Fixture artist - Fixture song - Fixture album"]
    assert processed == [[audio]]

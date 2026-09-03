from pathlib import Path

from muzik.commands import workflow
from muzik.core import cache as cache_mod
from muzik.core.sources.base import ResolvedTrack


def test_cli_routes_spotify_export_to_soulseek_without_media_download(
    tmp_path: Path,
    monkeypatch,
) -> None:
    """The CLI accepts an export file and sends each track's structured
    metadata directly to Soulseek — never a joined text query, and never
    yt-dlp."""
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    export = Path("tests/fixtures/spotify/playlist_v1.json").resolve()
    audio = tmp_path / "downloads" / "fixture.flac"
    audio.parent.mkdir()
    audio.write_bytes(b"audio")
    acquired_tracks: list[ResolvedTrack] = []
    processed: list[list[Path]] = []

    def acquire_track(track: ResolvedTrack, **_kwargs: object) -> list[Path]:
        acquired_tracks.append(track)
        return [audio]

    monkeypatch.setattr(workflow, "_acquire_track_from_soulseek", acquire_track)
    monkeypatch.setattr(
        workflow,
        "_acquire_from_soulseek",
        lambda *_a, **_k: (_ for _ in ()).throw(
            AssertionError("Spotify tracks must use structured acquisition")
        ),
    )
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

    assert len(acquired_tracks) == 1
    track = acquired_tracks[0]
    assert track.artist == "Fixture artist"
    assert track.title == "Fixture song"
    assert track.album == "Fixture album"
    assert processed == [[audio]]


def test_cli_routes_spotify_csv_export_to_soulseek_with_isrc_evidence(
    tmp_path: Path,
    monkeypatch,
) -> None:
    """A CSV export (e.g. Exportify) routes through the same structured
    acquisition path as JSON, keeping its ISRC as identity evidence."""
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    export = Path("tests/fixtures/spotify/exportify.csv").resolve()
    audio = tmp_path / "downloads" / "fixture.flac"
    audio.parent.mkdir()
    audio.write_bytes(b"audio")
    acquired_tracks: list[ResolvedTrack] = []

    def acquire_track(track: ResolvedTrack, **_kwargs: object) -> list[Path]:
        acquired_tracks.append(track)
        return [audio]

    monkeypatch.setattr(workflow, "_acquire_track_from_soulseek", acquire_track)
    monkeypatch.setattr(
        workflow,
        "_acquire_from_soulseek",
        lambda *_a, **_k: (_ for _ in ()).throw(
            AssertionError("Spotify tracks must use structured acquisition")
        ),
    )
    monkeypatch.setattr(
        workflow,
        "_process_audio_files",
        lambda *, audio_inputs, **_kwargs: None,
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

    assert len(acquired_tracks) == 1
    track = acquired_tracks[0]
    assert track.artist == "Fixture artist"
    assert track.title == "Fixture song"
    assert track.source_metadata.get("isrc") == "USFIXTURE0001"

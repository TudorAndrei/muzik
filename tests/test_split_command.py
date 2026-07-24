from pathlib import Path

import pytest
import typer

from muzik.commands import split
from muzik.core import cache as cache_mod
from muzik.core.chapters import Chapter


def _configure_split(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(
        split,
        "find_chapters",
        lambda _: [Chapter(index=1, start=0, end=None, title="Track")],
    )
    monkeypatch.setattr(
        split,
        "extract_metadata",
        lambda _: {"artist": "Artist", "album": "Album", "year": "2026"},
    )
    monkeypatch.setattr(split, "display_chapter_table", lambda *args, **kwargs: None)


def _audio_with_chapters(tmp_path: Path, stem: str = "album") -> tuple[Path, Path]:
    audio = tmp_path / f"{stem}.flac"
    chapters = tmp_path / f"{stem}.chapters.txt"
    audio.write_bytes(b"audio")
    chapters.write_text("00:00 Track\n")
    return audio, chapters


def test_split_cache_hit_preserves_existing_output_and_source(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _configure_split(monkeypatch)
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    audio, chapters = _audio_with_chapters(tmp_path)
    output = tmp_path / "existing-output"
    retained = output / "keep.flac"
    output.mkdir()
    retained.write_bytes(b"keep")
    cache_mod.set(cache_mod.split_cache_key(audio, chapters), str(output))

    with pytest.raises(typer.Exit) as result:
        split.split_cmd(
            path=audio,
            output=output,
            review=False,
            jobs=0,
            keep_source=False,
            force=False,
        )

    assert result.value.exit_code == 0
    assert audio.exists()
    assert chapters.exists()
    assert retained.read_bytes() == b"keep"


def test_split_requires_force_to_replace_nonempty_output(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _configure_split(monkeypatch)
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    audio, _ = _audio_with_chapters(tmp_path)
    output = tmp_path / "unrelated-output"
    retained = output / "do-not-delete.flac"
    output.mkdir()
    retained.write_bytes(b"keep")

    with pytest.raises(typer.Exit) as result:
        split.split_cmd(
            path=audio,
            output=output,
            review=False,
            jobs=0,
            keep_source=False,
            force=False,
        )

    assert result.value.exit_code == 1
    assert audio.exists()
    assert retained.read_bytes() == b"keep"


def test_split_force_replaces_output_and_empty_output_is_allowed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _configure_split(monkeypatch)
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    audio, _ = _audio_with_chapters(tmp_path)
    output = tmp_path / "output"
    old_file = output / "old.flac"
    output.mkdir()
    old_file.write_bytes(b"old")

    def split_track(audio_path, output_dir, chapter, metadata, track_count):
        (output_dir / "01-Track.flac").write_bytes(b"new")
        return True, chapter.title

    monkeypatch.setattr(split, "_split_track", split_track)

    split.split_cmd(
        path=audio, output=output, force=True, keep_source=True, jobs=1, review=False
    )

    assert not old_file.exists()
    assert (output / "01-Track.flac").read_bytes() == b"new"
    assert audio.exists()

    second_audio, second_chapters = _audio_with_chapters(tmp_path, stem="second-album")
    second_chapters.unlink()
    empty_output = tmp_path / "empty-output"
    empty_output.mkdir()
    split.split_cmd(
        path=second_audio,
        output=empty_output,
        keep_source=True,
        jobs=1,
        review=False,
        force=False,
    )
    assert (empty_output / "01-Track.flac").exists()


def test_split_failure_preserves_source_and_does_not_cache(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _configure_split(monkeypatch)
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    audio, chapters = _audio_with_chapters(tmp_path)
    output = tmp_path / "output"
    monkeypatch.setattr(split, "_split_track", lambda *args: (False, "Track"))

    with pytest.raises(typer.Exit) as result:
        split.split_cmd(path=audio, output=output, jobs=1, review=False)

    assert result.value.exit_code == 1
    assert audio.exists()
    assert chapters.exists()
    assert cache_mod.get(cache_mod.split_cache_key(audio, chapters)) is None

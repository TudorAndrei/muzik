"""Regression tests: sidecar lookup must survive a dot inside the filename."""

from pathlib import Path

from muzik.core.chapters import find_chapters, sidecar_path
from muzik.core.musicbrainz import is_searchable_album


def test_sidecar_path_keeps_dotted_stem() -> None:
    # "Vol. 1" has an internal dot; with_suffix would rewrite ". 1 [id]".
    audio = Path("/x/Roadstar Mix Vol. 1 [Z7weUThpPpk].opus")
    assert sidecar_path(audio, ".chapters.txt") == Path(
        "/x/Roadstar Mix Vol. 1 [Z7weUThpPpk].chapters.txt"
    )
    assert sidecar_path(audio, ".info.json") == Path(
        "/x/Roadstar Mix Vol. 1 [Z7weUThpPpk].info.json"
    )


def test_find_chapters_finds_dotted_name_sidecar(tmp_path) -> None:
    audio = tmp_path / "Roadstar Mix Vol. 1 [Z7weUThpPpk].opus"
    audio.write_bytes(b"")
    sidecar_path(audio, ".chapters.txt").write_text(
        "00:00 Intro\n03:00 Second\n", encoding="utf-8"
    )
    chapters = find_chapters(audio)
    assert [c.title for c in chapters] == ["Intro", "Second"]


def test_is_searchable_album_rejects_placeholders() -> None:
    assert is_searchable_album("Blue Waters") is True
    assert is_searchable_album("Unknown Album") is False
    assert is_searchable_album("unknown") is False
    assert is_searchable_album("") is False

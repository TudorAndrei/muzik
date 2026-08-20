"""Tests for placing a downloaded thumbnail as album cover art."""

from pathlib import Path

from muzik.core import splitter


def test_place_cover_copies_jpg_thumbnail(tmp_path: Path) -> None:
    base = tmp_path / "Title [dQw4w9WgXcQ]"
    base.with_suffix(".jpg").write_bytes(b"JPEGDATA")
    output = tmp_path / "album"
    output.mkdir()

    splitter._place_cover(base, output)

    cover = output / "cover.jpg"
    assert cover.exists() and cover.read_bytes() == b"JPEGDATA"


def test_place_cover_prefers_jpg_over_webp(tmp_path: Path) -> None:
    base = tmp_path / "Title [id]"
    base.with_suffix(".jpg").write_bytes(b"JPG")
    base.with_suffix(".webp").write_bytes(b"WEBP")
    output = tmp_path / "album"
    output.mkdir()

    splitter._place_cover(base, output)

    assert (output / "cover.jpg").read_bytes() == b"JPG"
    assert not (output / "cover.webp").exists()


def test_place_cover_noop_without_thumbnail(tmp_path: Path) -> None:
    base = tmp_path / "Title [id]"
    output = tmp_path / "album"
    output.mkdir()

    splitter._place_cover(base, output)

    assert list(output.iterdir()) == []

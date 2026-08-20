"""Tests for the downloaded-audio inventory and archive seeding."""

from pathlib import Path

from muzik.core.library import (
    downloaded_ids,
    scan_downloads,
    seed_archive_from_downloads,
    youtube_id_from_name,
)


def _make_audio(directory: Path, name: str, data: bytes = b"x") -> Path:
    directory.mkdir(parents=True, exist_ok=True)
    path = directory / name
    path.write_bytes(data)
    return path


def test_youtube_id_from_name_reads_bracketed_id() -> None:
    assert youtube_id_from_name("Some Title [dQw4w9WgXcQ].m4a") == "dQw4w9WgXcQ"
    assert youtube_id_from_name("no id here.flac") is None


def test_scan_downloads_lists_audio_sorted_by_title(tmp_path: Path) -> None:
    _make_audio(tmp_path, "Beta [aaaaaaaaaaa].m4a")
    _make_audio(tmp_path, "Alpha [bbbbbbbbbbb].flac")
    _make_audio(tmp_path, "notes.txt")  # ignored, not audio

    items = scan_downloads(tmp_path)

    assert [item.title for item in items] == ["Alpha", "Beta"]
    assert items[0].youtube_id == "bbbbbbbbbbb"
    assert items[0].ext == "flac"


def test_scan_downloads_empty_when_missing(tmp_path: Path) -> None:
    assert scan_downloads(tmp_path / "absent") == []


def test_downloaded_ids_collects_every_bracketed_id(tmp_path: Path) -> None:
    _make_audio(tmp_path, "One [11111111111].mp3")
    _make_audio(tmp_path, "Two [22222222222].mp3")
    _make_audio(tmp_path, "Local track.wav")  # no id

    assert downloaded_ids(tmp_path) == {"11111111111", "22222222222"}


def test_seed_archive_adds_missing_ids(tmp_path: Path) -> None:
    downloads = tmp_path / "downloads"
    _make_audio(downloads, "One [11111111111].mp3")
    _make_audio(downloads, "Two [22222222222].mp3")
    archive = tmp_path / "cache" / "ytdlp_archive_PL.txt"
    archive.parent.mkdir(parents=True)
    archive.write_text("youtube 11111111111\n")

    added = seed_archive_from_downloads(archive, downloads)

    assert added == 1
    lines = set(archive.read_text().splitlines())
    assert lines == {"youtube 11111111111", "youtube 22222222222"}


def test_seed_archive_is_idempotent(tmp_path: Path) -> None:
    downloads = tmp_path / "downloads"
    _make_audio(downloads, "One [11111111111].mp3")
    archive = tmp_path / "ytdlp_archive_PL.txt"

    first = seed_archive_from_downloads(archive, downloads)
    second = seed_archive_from_downloads(archive, downloads)

    assert first == 1
    assert second == 0
    assert archive.read_text().splitlines() == ["youtube 11111111111"]

from pathlib import Path

from muzik.core.audio import extract_metadata
from muzik.core.metadata import (
    metadata_sidecar_for,
    read_muzik_metadata,
    write_muzik_metadata,
)


def test_write_and_read_file_metadata_sidecar(tmp_path: Path) -> None:
    audio = tmp_path / "Artist - Song.flac"
    audio.write_bytes(b"")

    sidecar = write_muzik_metadata(
        audio,
        {
            "source": "soulseek",
            "source_id": "peer:file",
            "resolved": {
                "title": "Song",
                "artist": "Artist",
                "album": "Album",
                "year": "1999",
            },
        },
    )

    assert sidecar == tmp_path / "Artist - Song.muzik.json"
    data = read_muzik_metadata(audio)
    assert data is not None
    assert data["version"] == 1
    assert data["source"] == "soulseek"


def test_directory_metadata_sidecar_path(tmp_path: Path) -> None:
    assert metadata_sidecar_for(tmp_path) == tmp_path / ".muzik.json"


def test_extract_metadata_prefers_muzik_sidecar_over_info_json(tmp_path: Path) -> None:
    audio = tmp_path / "Video Title.flac"
    audio.write_bytes(b"")
    audio.with_suffix(".info.json").write_text(
        '{"title": "YouTube Title", "uploader": "Uploader"}',
        encoding="utf-8",
    )
    write_muzik_metadata(
        audio,
        {
            "source": "soulseek",
            "resolved": {
                "title": "Sidecar Title",
                "artist": "Sidecar Artist",
                "album": "Sidecar Album",
                "year": "2001-01-01",
            },
        },
    )

    assert extract_metadata(audio) == {
        "title": "Sidecar Title",
        "artist": "Sidecar Artist",
        "album": "Sidecar Album",
        "year": "2001",
    }

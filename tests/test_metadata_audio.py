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


def test_extract_metadata_removes_youtube_noise_from_album_name(
    tmp_path: Path,
) -> None:
    audio = tmp_path / "Forestal Tape.opus"
    audio.write_bytes(b"")
    write_muzik_metadata(
        audio,
        {
            "source": "youtube",
            "resolved": {
                "title": "Forestal Tape",
                "artist": "Las Luces Primeras",
                "album": "Forestal Tape (FULL ALBUM)",
                "year": "2018",
            },
        },
    )

    assert extract_metadata(audio)["album"] == "Forestal Tape"


def test_extract_metadata_uses_info_json_when_muzik_metadata_is_empty(
    tmp_path: Path,
) -> None:
    audio = tmp_path / "Hiromasa Suzuki - High-Flying [video].opus"
    audio.write_bytes(b"")
    audio.with_suffix(".info.json").write_text(
        '{"title": "Hiromasa Suzuki - High-Flying (1976) (Full Album)", '
        '"uploader": "CanuDigit", "upload_date": "20180102"}',
        encoding="utf-8",
    )
    write_muzik_metadata(
        audio,
        {
            "source": "youtube",
            "resolved": {},
            "candidate": {"metadata": {}},
        },
    )

    assert extract_metadata(audio) == {
        "title": "Hiromasa Suzuki - High-Flying (1976) (Full Album)",
        "artist": "Hiromasa Suzuki",
        "album": "High-Flying",
        "year": "1976",
    }


def test_extract_metadata_reads_opus_stream_tags(tmp_path: Path, monkeypatch) -> None:
    audio = tmp_path / "track.opus"
    audio.write_bytes(b"")
    monkeypatch.setattr(
        "muzik.core.audio.probe",
        lambda path: {
            "format": {"tags": {}},
            "streams": [
                {
                    "codec_type": "audio",
                    "tags": {
                        "TITLE": "High-Flying",
                        "ARTIST": "Hiromasa Suzuki",
                        "ALBUM": "High-Flying",
                        "DATE": "1976",
                    },
                }
            ],
        },
    )

    assert extract_metadata(audio) == {
        "title": "High-Flying",
        "artist": "Hiromasa Suzuki",
        "album": "High-Flying",
        "year": "1976",
    }

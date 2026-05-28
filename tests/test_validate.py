from pathlib import Path

import pytest
import typer

from muzik.commands import validate
from muzik.core.metadata import write_muzik_metadata


def test_validate_accepts_muzik_json_sidecar(tmp_path: Path) -> None:
    sidecar = write_muzik_metadata(
        tmp_path / "Album",
        {
            "source": "soulseek",
            "source_id": "peer:/Album",
            "candidate": {
                "quality": {
                    "format": "flac",
                    "lossless": True,
                }
            },
        },
    )

    validate.validate_cmd(sidecar, verbose=True)


def test_validate_rejects_invalid_muzik_json(tmp_path: Path) -> None:
    sidecar = tmp_path / "bad.muzik.json"
    sidecar.write_text("[]", encoding="utf-8")

    with pytest.raises(typer.Exit) as exc:
        validate.validate_cmd(sidecar, verbose=True)

    assert exc.value.exit_code == 1


def test_validate_audio_reports_missing_metadata_warning(
    tmp_path: Path,
    monkeypatch,
) -> None:
    audio = tmp_path / "Track.flac"
    audio.write_bytes(b"")

    monkeypatch.setattr(
        validate,
        "probe",
        lambda path: {
            "format": {"duration": "1.0"},
            "streams": [{"codec_name": "flac"}],
        },
    )

    validate.validate_cmd(audio, verbose=True)


def test_validate_warns_when_album_sidecar_expects_more_files(
    tmp_path: Path,
    capsys,
) -> None:
    album = tmp_path / "Album"
    album.mkdir()
    (album / "01 One.flac").write_bytes(b"")
    (album / ".muzik.json").write_text(
        """
{
  "version": 1,
  "source": "soulseek",
  "source_id": "peer:/Album",
  "candidate": {
    "files": [
      {"name": "01 One.flac"},
      {"name": "02 Two.flac"}
    ],
    "quality": {
      "format": "flac",
      "lossless": true
    }
  }
}
""",
        encoding="utf-8",
    )

    data = validate.read_muzik_metadata(album / ".muzik.json")
    assert data is not None
    assert validate._album_completeness_warnings(album / ".muzik.json", data) == [
        "album appears incomplete (1/2 audio files)"
    ]

    validate.validate_cmd(album, verbose=True)

    captured = capsys.readouterr()
    assert "warnings" in captured.err

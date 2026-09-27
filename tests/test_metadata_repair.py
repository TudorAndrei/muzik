import json
from pathlib import Path
from types import SimpleNamespace

from muzik.core import metadata_repair


def _fake_tags(monkeypatch, tags: dict[Path, dict]) -> list[Path]:
    saved: list[Path] = []

    def write(path: str, payload: str) -> None:
        tags[Path(path)] = json.loads(payload)
        saved.append(Path(path))

    monkeypatch.setattr(
        metadata_repair,
        "_native",
        SimpleNamespace(
            read_audio_tags=lambda path: tags[Path(path)],
            write_audio_tags=write,
            TagsError=ValueError,
        ),
    )
    return saved


def test_repair_placeholder_album_tags_from_split_folder(
    tmp_path: Path, monkeypatch
) -> None:
    album = tmp_path / "Kohsuke Mine - Sunshower (1976) (Full Album) [U3AXfrVMfbg]"
    album.mkdir()
    tracks = [album / f"{index:02d}-track.opus" for index in range(1, 5)]
    tags = {
        track: {
            "fields": {
                "artist": "Unknown Artist",
                "albumartist": "Unknown Artist",
                "album": "Unknown Album",
            },
            "lists": {},
            "custom": {"source": "video"},
        }
        for track in tracks
    }
    for track in tracks:
        track.write_bytes(b"audio")
    saved = _fake_tags(monkeypatch, tags)

    result = metadata_repair.repair_placeholder_album_tags(album)

    assert result.updated_files == 4
    assert (result.artist, result.album, result.year) == (
        "Kohsuke Mine",
        "Sunshower",
        "1976",
    )
    assert len(saved) == 4
    for item in tags.values():
        assert item["fields"] == {
            "artist": "Kohsuke Mine",
            "albumartist": "Kohsuke Mine",
            "album": "Sunshower",
            "date": "1976",
        }
        assert item["custom"] == {"source": "video"}


def test_repair_placeholder_album_tags_keeps_real_tags(
    tmp_path: Path, monkeypatch
) -> None:
    album = tmp_path / "Kohsuke Mine - Sunshower (1976) [U3AXfrVMfbg]"
    album.mkdir()
    track = album / "01-track.opus"
    track.write_bytes(b"audio")
    tags = {
        track: {
            "fields": {
                "artist": "Kohsuke Mine",
                "albumartist": "Kohsuke Mine",
                "album": "Sunshower",
                "date": "1976",
            },
            "lists": {},
            "custom": {},
        }
    }
    saved = _fake_tags(monkeypatch, tags)
    result = metadata_repair.repair_placeholder_album_tags(album)
    assert result.updated_files == 0
    assert saved == []

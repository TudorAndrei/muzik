from pathlib import Path

from muzik.core import metadata_repair


class FakeMediaFile:
    def __init__(self) -> None:
        self.artist = "Unknown Artist"
        self.albumartist = "Unknown Artist"
        self.album = "Unknown Album"
        self.year = None
        self.saved = False

    def save(self) -> None:
        self.saved = True


def test_repair_placeholder_album_tags_from_split_folder(
    tmp_path: Path, monkeypatch
) -> None:
    album = tmp_path / "Kohsuke Mine - Sunshower (1976) (Full Album) [U3AXfrVMfbg]"
    album.mkdir()
    tracks = [album / f"{index:02d}-track.opus" for index in range(1, 5)]
    media = {track: FakeMediaFile() for track in tracks}
    for track in tracks:
        track.write_bytes(b"audio")
    monkeypatch.setattr(
        metadata_repair,
        "MediaFile",
        lambda path: media[Path(path)],
    )

    result = metadata_repair.repair_placeholder_album_tags(album)

    assert result.updated_files == 4
    assert result.artist == "Kohsuke Mine"
    assert result.album == "Sunshower"
    assert result.year == "1976"
    for item in media.values():
        assert item.artist == "Kohsuke Mine"
        assert item.albumartist == "Kohsuke Mine"
        assert item.album == "Sunshower"
        assert item.year == 1976
        assert item.saved is True


def test_repair_placeholder_album_tags_keeps_real_tags(
    tmp_path: Path, monkeypatch
) -> None:
    album = tmp_path / "Kohsuke Mine - Sunshower (1976) [U3AXfrVMfbg]"
    album.mkdir()
    track = album / "01-track.opus"
    track.write_bytes(b"audio")
    item = FakeMediaFile()
    item.artist = "Kohsuke Mine"
    item.albumartist = "Kohsuke Mine"
    item.album = "Sunshower"
    item.year = 1976
    monkeypatch.setattr(metadata_repair, "MediaFile", lambda path: item)

    result = metadata_repair.repair_placeholder_album_tags(album)

    assert result.updated_files == 0
    assert item.saved is False

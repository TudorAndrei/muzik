"""Check the selected audio tag backend with real audio files."""

from pathlib import Path
import shutil
from types import SimpleNamespace

from mediafile import MediaFile
import pytest

from muzik.core import audio, quality
from muzik.core import import_service as service
from muzik.core.import_models import ImportOptions
from muzik.core.import_models import LogEvent, RecordingImportEventEmitter

FIXTURES = Path(__file__).resolve().parents[1] / "rust/crates/muzik-tags/tests/fixtures"


def test_native_probe_quality_and_metadata(tmp_path) -> None:
    path = tmp_path / "track.flac"
    shutil.copyfile(FIXTURES / "mediafile.flac", path)
    data = audio.probe(path)
    assert float(data["format"]["duration"]) == 0.3
    assert data["streams"][0]["sample_rate"] == 48_000
    assert data["streams"][0]["codec_name"] == "flac"
    assert audio.get_duration(path) == 0.3
    assert audio.extract_metadata(path) == {
        "title": "Tide & Stone",
        "artist": "Mara Vale",
        "album": "Night Lines",
        "year": "2021",
    }
    measured = quality.measure_quality(path)
    assert measured is not None
    assert measured.format == "flac"
    assert measured.lossless
    assert measured.sample_rate == 48_000
    assert measured.bit_depth == 16
    assert measured.channels == 1
    assert measured.measured


@pytest.mark.parametrize(
    ("suffix", "codec", "lossless"),
    [
        ("mp3", "mp3", False),
        ("flac", "flac", True),
        ("m4a", "aac", False),
        ("opus", "opus", False),
        ("ogg", "ogg", False),
    ],
)
def test_native_quality_for_each_format(
    tmp_path, suffix: str, codec: str, lossless: bool
) -> None:
    path = tmp_path / f"track.{suffix}"
    shutil.copyfile(FIXTURES / f"mediafile.{suffix}", path)
    measured = quality.measure_quality(path)
    assert measured is not None
    assert measured.format == codec
    assert measured.lossless is lossless
    assert measured.sample_rate == 48_000
    assert measured.duration is not None
    assert 0.25 <= measured.duration <= 0.4


def test_native_tag_only_writes_library_fields_and_cover(tmp_path, monkeypatch) -> None:
    from muzik import _native

    path = tmp_path / "track.flac"
    shutil.copyfile(FIXTURES / "blank.flac", path)
    shutil.copyfile(FIXTURES / "cover.png", tmp_path / "cover.png")
    values = {
        "title": "New Song",
        "artist": "New Artist",
        "album": "New Album",
        "albumartist": "New Artist",
        "track": 2,
        "year": 2022,
        "month": 5,
        "day": 6,
        "mb_albumid": "33333333-3333-4333-8333-333333333333",
        "comp": False,
    }
    item = SimpleNamespace(path=str(path).encode(), get=values.get)
    library = SimpleNamespace(directory=str(tmp_path), items=lambda: [item])
    monkeypatch.setattr(service, "NativeLibrary", lambda _: library)
    service.write_library_tags(path, ImportOptions(paths=[path]))

    media = MediaFile(path)
    assert (media.title, media.artist, media.album) == (
        "New Song",
        "New Artist",
        "New Album",
    )
    assert (media.year, media.month, media.day) == (2022, 5, 6)
    assert media.mb_albumid == values["mb_albumid"]
    assert media.images
    assert _native.audio_has_front_cover(str(path))


def test_native_tag_only_dry_run_keeps_file(tmp_path, monkeypatch) -> None:
    path = tmp_path / "track.flac"
    shutil.copyfile(FIXTURES / "blank.flac", path)
    before = path.read_bytes()
    item = SimpleNamespace(path=str(path).encode(), get={"title": "New Song"}.get)
    monkeypatch.setattr(
        service,
        "NativeLibrary",
        lambda _: SimpleNamespace(directory=str(tmp_path), items=lambda: [item]),
    )
    service.write_library_tags(path, ImportOptions(paths=[path], dry_run=True))
    assert path.read_bytes() == before


def test_tag_only_resolves_relative_item_path(tmp_path, monkeypatch) -> None:
    path = tmp_path / "track.flac"
    shutil.copyfile(FIXTURES / "blank.flac", path)
    item = SimpleNamespace(path=b"track.flac", get={"title": "New Song"}.get)
    library = SimpleNamespace(directory=str(tmp_path), items=lambda: [item])
    monkeypatch.setattr(service, "NativeLibrary", lambda _: library)
    service.write_library_tags(path, ImportOptions(paths=[path]))
    assert MediaFile(path).title == "New Song"


def test_tag_only_dry_run_reports_selected_items(tmp_path, monkeypatch) -> None:
    path = tmp_path / "track.flac"
    shutil.copyfile(FIXTURES / "blank.flac", path)
    item = SimpleNamespace(path=b"track.flac", get={"title": "New Song"}.get)
    library = SimpleNamespace(directory=str(tmp_path), items=lambda: [item])
    monkeypatch.setattr(service, "NativeLibrary", lambda _: library)
    events = RecordingImportEventEmitter()
    service.organize_paths(
        ImportOptions(paths=[path], dry_run=True),
        tag_only=True,
        events=events,
    )
    assert any(
        isinstance(event, LogEvent) and "1 library items" in event.message
        for event in events.events
    )

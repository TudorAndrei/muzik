"""Write fixed mediafile audio tags and check files written by Rust.

Run with ``uv run python scripts/beets_fixtures/tags.py``. Audio fixtures need ffmpeg.
Set ``--check-rust DIR`` to read five files written by muzik-tags.
"""

from __future__ import annotations

import argparse
import importlib.metadata
import json
import subprocess
from pathlib import Path

import mediafile

mediafile.MediaFile.add_field(
    "muzik_mood",
    mediafile.MediaField(
        mediafile.MP3DescStorageStyle("MUZIK_MOOD"),
        mediafile.MP4StorageStyle("----:com.apple.iTunes:MUZIK_MOOD"),
        mediafile.StorageStyle("MUZIK_MOOD"),
    ),
)

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "rust/crates/muzik-tags/tests/fixtures"
FORMATS = {
    "mp3": ["-c:a", "libmp3lame", "-q:a", "9"],
    "flac": ["-c:a", "flac"],
    "m4a": ["-c:a", "aac", "-b:a", "48k"],
    "opus": ["-c:a", "libopus", "-b:a", "32k"],
    "ogg": ["-c:a", "vorbis", "-strict", "-2", "-q:a", "0"],
}
VALUES = {
    "title": "Tide & Stone",
    "muzik_mood": "calm",
    "artist": "Mara Vale",
    "artists": ["Mara Vale", "Lio Hart"],
    "album": "Night Lines",
    "albumartist": "Mara Vale",
    "albumartists": ["Mara Vale", "Lio Hart"],
    "track": 2,
    "tracktotal": 9,
    "disc": 1,
    "disctotal": 2,
    "year": 2021,
    "month": 4,
    "day": 7,
    "original_year": 2019,
    "original_month": 11,
    "original_day": 3,
    "mb_trackid": "11111111-1111-4111-8111-111111111111",
    "mb_releasetrackid": "22222222-2222-4222-8222-222222222222",
    "mb_workid": "77777777-7777-4777-8777-777777777777",
    "mb_albumid": "33333333-3333-4333-8333-333333333333",
    "mb_releasegroupid": "44444444-4444-4444-8444-444444444444",
    "mb_artistid": "55555555-5555-4555-8555-555555555555",
    "mb_albumartistid": "66666666-6666-4666-8666-666666666666",
    "label": "Harbor Records",
    "catalognum": "HR-204",
    "country": "GB",
    "media": "Digital Media",
    "albumdisambig": "2019 remaster",
    "comp": True,
    "rg_track_gain": -5.25,
    "rg_track_peak": 0.912345,
    "rg_album_gain": -4.5,
    "rg_album_peak": 0.987654,
}
READ_FIELDS = tuple(VALUES)


def projection(path: Path) -> dict[str, object]:
    audio = mediafile.MediaFile(path)
    return {key: getattr(audio, key) for key in READ_FIELDS}


def generate() -> None:
    FIXTURES.mkdir(parents=True, exist_ok=True)
    expected = {}
    for suffix, codec in FORMATS.items():
        path = FIXTURES / f"blank.{suffix}"
        subprocess.run(
            [
                "ffmpeg",
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=0.3",
                "-ac",
                "2" if suffix == "ogg" else "1",
                "-ar",
                "48000",
                *codec,
                str(path),
            ],
            check=True,
        )
        tagged = FIXTURES / f"mediafile.{suffix}"
        tagged.write_bytes(path.read_bytes())
        audio = mediafile.MediaFile(tagged)
        for key, value in VALUES.items():
            setattr(audio, key, value)
        audio.save()
        expected[suffix] = projection(tagged)
    (FIXTURES / "mediafile_tags.json").write_text(
        json.dumps(
            {
                "beets_version": importlib.metadata.version("beets"),
                "mediafile_version": importlib.metadata.version("mediafile"),
                "files": expected,
            },
            indent=2,
            sort_keys=True,
        )
        + "\n"
    )


def check_rust(directory: Path) -> None:
    expected = json.loads((FIXTURES / "mediafile_tags.json").read_text())["files"]
    for suffix in FORMATS:
        actual = projection(directory / f"rust.{suffix}")
        if actual != expected[suffix]:
            raise AssertionError(
                f"{suffix}: expected {expected[suffix]!r}; got {actual!r}"
            )
    print("mediafile read all five Rust-written files")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--check-rust", type=Path)
    args = parser.parse_args()
    if args.check_rust:
        check_rust(args.check_rust)
    else:
        generate()

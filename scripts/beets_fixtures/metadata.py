"""Record a MusicBrainz release and the beets AlbumInfo made from it."""

from __future__ import annotations

import json
from pathlib import Path
from urllib.parse import urlencode
from urllib.request import Request, urlopen

import beets
from beetsplug._utils.musicbrainz import MusicBrainzAPI, RELEASE_INCLUDES
from beetsplug.musicbrainz import MusicBrainzPlugin


RELEASE_ID = "76df3287-6cda-33eb-8e9a-044b5e15ffdd"
USER_AGENT = "muzik-fixture/0.1 (https://github.com/TudorAndrei/muzik)"


def main() -> None:
    params = urlencode({"inc": "+".join(RELEASE_INCLUDES), "fmt": "json"})
    url = f"https://musicbrainz.org/ws/2/release/{RELEASE_ID}?{params}"
    request = Request(url, headers={"User-Agent": USER_AGENT})
    with urlopen(request, timeout=30) as response:
        raw = json.load(response)

    normalized = MusicBrainzAPI._normalize_data(raw)
    album = MusicBrainzPlugin().album_info(normalized)
    expected = {
        "id": album.album_id,
        "title": album.album,
        "artist": album.artist,
        "release_group_id": album.releasegroup_id,
        "year": album.year,
        "country": album.country,
        "media": album.media,
        "label": album.label,
        "catalog_number": album.catalognum,
        "disambiguation": album.albumdisambig,
        "is_various_artists": album.va,
        "tracks": [
            {
                "recording_id": track.track_id,
                "release_track_id": track.release_track_id,
                "title": track.title,
                "artist": track.artist,
                "length_seconds": track.length,
                "index": track.index,
                "medium": track.medium,
                "medium_index": track.medium_index,
            }
            for track in album.tracks
        ],
    }
    fixture = {
        "beets_version": beets.__version__,
        "musicbrainz_release_id": RELEASE_ID,
        "raw": raw,
        "expected": expected,
    }
    destination = (
        Path(__file__).resolve().parents[2]
        / "rust/crates/muzik-metadata/tests/fixtures/release.json"
    )
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(
        json.dumps(fixture, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )


if __name__ == "__main__":
    main()

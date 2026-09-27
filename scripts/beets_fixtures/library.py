"""Create a beets SQLite library for native read tests."""

import json
import os
from pathlib import Path

import beets
from beets import config
from beets.library import Item, Library


ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "rust/crates/muzik-library/tests/fixtures"
DATABASE = FIXTURES / "library.db"


def main() -> None:
    FIXTURES.mkdir(parents=True, exist_ok=True)
    DATABASE.unlink(missing_ok=True)
    config["create_backup_before_migrations"].set(False)
    library = Library(str(DATABASE), "/fixture/music")
    first = Item(
        path=os.fsencode("/fixture/music/Artist/Album/01 Track.mp3"),
        title="Track",
        artist="Artist",
        album="Album",
        albumartist="Artist",
        track=1,
        length=183.5,
    )
    first.muzik_source_id = "video-123"
    library.add(first)
    first.store()
    album = library.add_album([first])
    album.fixture_note = "album-flex"
    album.store()

    second = Item(
        path=os.fsencode("/fixture/music/Loose/Single.flac"),
        title="Single",
        artist="Solo",
        track=1,
    )
    second.muzik_source_id = "video-456"
    library.add(second)
    second.store()
    expected = {
        "beets_version": beets.__version__,
        "album_id": album.id,
        "album_title": album.album,
        "album_attribute": album.fixture_note,
        "first_id": first.id,
        "first_title": first.title,
        "first_source_id": first.muzik_source_id,
        "first_length": first.length,
        "second_id": second.id,
        "second_title": second.title,
        "second_source_id": second.muzik_source_id,
    }
    library._close()
    (FIXTURES / "library.json").write_text(
        json.dumps(expected, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )


if __name__ == "__main__":
    main()

"""Record path destinations from beets 2.13.1 for the Rust importer."""

from __future__ import annotations

import json
import tempfile
from pathlib import Path

import beets
from beets import config, util
from beets.library import Item, Library


OUTPUT = (
    Path(__file__).resolve().parents[2]
    / "rust/crates/muzik-import/tests/fixtures/paths.json"
)


def main() -> None:
    config.clear()
    config.read(user=False)
    config["paths"] = {
        "default": "$albumartist/$album%aunique{}/$track $title",
        "comp": "Compilations/$album%aunique{}/$track $title",
        "singleton": "Non-Album/$artist/$title",
    }
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        library = Library(root / "library.db", directory=str(root / "music"))
        first = Item(
            path=str(root / "first.flac"),
            albumartist="Portishead",
            artist="Portishead",
            album="Dummy",
            title="Mysterons",
            track=1,
            tracktotal=10,
            year=1994,
        )
        library.add_album([first])
        second = Item(
            path=str(root / "second.flac"),
            albumartist="Portishead",
            artist="Portishead",
            album="Dummy",
            title="Sour Times",
            track=2,
            tracktotal=10,
            year=2008,
        )
        library.add_album([second])
        compilation = Item(
            path=str(root / "comp.flac"),
            albumartist="Various Artists",
            artist="Björk",
            album="Sampler",
            title="Hidden Place",
            track=1,
            tracktotal=9,
            comp=True,
        )
        library.add_album([compilation])
        singleton = Item(
            path=str(root / "single.flac"),
            artist="Björk",
            title="Hidden Place",
            singleton=True,
        )
        library.add(singleton)
        cases = []
        for name, item in [
            ("first", first),
            ("second", second),
            ("compilation", compilation),
            ("singleton", singleton),
        ]:
            cases.append(
                {
                    "name": name,
                    "path_format": (
                        "singleton"
                        if name == "singleton"
                        else "comp"
                        if name == "compilation"
                        else "default"
                    ),
                    "album_id": item.album_id,
                    "fields": {
                        "albumartist": item.albumartist,
                        "artist": item.artist,
                        "album": item.album,
                        "title": item.title,
                        "track": item.formatted(for_path=True)["track"],
                        "year": item.formatted(for_path=True)["year"],
                    },
                    "destination": item.destination(relative_to_libdir=True).decode(),
                }
            )
    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    sanitize = [
        {
            "subpath": subpath,
            "extension": extension,
            "destination": util.legalize_path(
                subpath, Library.get_replacements(), extension
            )[0],
        }
        for subpath, extension in [
            ("Artist/Bad: title?", ".FLAC"),
            (".Hidden/ -track. ", ".mp3"),
            ("Artist/Name\\slash", ".ogg"),
        ]
    ]
    OUTPUT.write_text(
        json.dumps(
            {
                "beets_version": beets.__version__,
                "cases": cases,
                "sanitization": sanitize,
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )


if __name__ == "__main__":
    main()

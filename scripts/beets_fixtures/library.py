"""Create a beets SQLite library for native read tests."""

import json
import os
from pathlib import Path

import beets
from beets import config
from beets.dbcore import query as beets_query
from beets.dbcore import sort as beets_sort
from beets.library import Item, Library
from beets.library.queries import parse_query_string


ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "rust/crates/muzik-library/tests/fixtures"
DATABASE = FIXTURES / "library.db"
QUERY_CASES = [
    "artist:Artist",
    "title::^T",
    "length:180..200",
    "length:3:00..3:10",
    "-artist:Artist",
    "^artist:Artist",
    'title:"Track Name"',
    "artist:Artist, artist:Solo",
    "title-",
    "title+",
    "title:Track artist:Artist",
    "title:=Track",
    "Artist",
    "muzik_source_id:video-456",
    "album:Album",
]


def query_fixture(library: Library) -> dict:
    cases = []
    for text in QUERY_CASES:
        parsed, sort = parse_query_string(text, Item)
        groups = (
            parsed.subqueries if isinstance(parsed, beets_query.OrQuery) else [parsed]
        )
        normalized_groups = []
        for group in groups:
            assert isinstance(group, beets_query.AndQuery)
            terms = []
            for term in group.subqueries:
                if isinstance(term, beets_query.TrueQuery):
                    continue
                negated = isinstance(term, beets_query.NotQuery)
                if negated:
                    term = term.subquery
                any_field = isinstance(term, beets_query.OrQuery)
                if any_field:
                    term = term.subqueries[0]
                assert isinstance(term, beets_query.FieldQuery)
                kind = type(term).__name__.removesuffix("Query")
                if kind == "Duration":
                    kind = "Numeric"
                if kind == "Match":
                    kind = "Exact"
                pattern = term.pattern
                if hasattr(pattern, "pattern"):
                    pattern = pattern.pattern
                terms.append(
                    {
                        "field": None if any_field else term.field_name,
                        "pattern": pattern,
                        "kind": kind,
                        "negated": negated,
                    }
                )
            normalized_groups.append(terms)
        sorts = (
            []
            if isinstance(sort, beets_sort.NullSort)
            else [{"field": sort.field, "ascending": sort.ascending}]
        )
        cases.append(
            {
                "text": text,
                "groups": normalized_groups,
                "sorts": sorts,
                "item_ids": [item.id for item in library.items(text)],
            }
        )
    return {"beets_version": beets.__version__, "cases": cases}


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
    queries = query_fixture(library)
    library._close()
    (FIXTURES / "library.json").write_text(
        json.dumps(expected, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    (FIXTURES / "query.json").write_text(
        json.dumps(queries, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )


if __name__ == "__main__":
    main()

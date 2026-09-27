"""Write string-distance fixtures from the installed beets release."""

from __future__ import annotations

import json
from pathlib import Path

import beets
from beets.autotag.distance import string_dist


CASES: tuple[tuple[str | None, str | None], ...] = (
    (None, None),
    (None, "Song"),
    ("Song", None),
    ("", ""),
    ("", "!!!"),
    ("", "Song"),
    ("Song", "Song"),
    ("SONG", "song"),
    ("A B-C!", "abc"),
    ("The Cure", "Cure, The"),
    ("A Tribe Called Quest", "Tribe Called Quest, A"),
    ("An Artist", "Artist, An"),
    ("The The", "The, The"),
    ("The Album", "Album"),
    ("The Album", "Album, The"),
    ("Simon & Garfunkel", "Simon and Garfunkel"),
    ("Rock & Roll", "Rock and Roll"),
    ("Single", ""),
    ("Song (Single)", "Song"),
    ("Song [EP]", "Song"),
    ("Song EP", "Song"),
    ("Song (feat. Guest)", "Song"),
    ("Song featuring Guest", "Song"),
    ("Song ft: Guest", "Song"),
    ("Song (live)", "Song"),
    ("Song [remaster]", "Song"),
    ("Song (live) [remaster]", "Song"),
    ("Song, pt. 2", "Song"),
    ("Song part two", "Song"),
    ("Song (feat. Guest)", "Song [live]"),
    ("Song (Acoustic)", "Song (Live)"),
    ("Beyoncé", "Beyonce"),
    ("Sigur Rós", "Sigur Ros"),
    ("Mötley Crüe", "Motley Crue"),
    ("São Paulo", "Sao Paulo"),
    ("東京", "Dong Jing"),
    ("Москва", "Moskva"),
    ("Ελλάδα", "Ellada"),
    ("Straße", "Strasse"),
    ("Æther", "Aether"),
    ("ø", "o"),
    ("🎵", ""),
    ("Line\nBreak", "Line Break"),
    ("(intro)", ""),
    ("[EP] (live)", ""),
)


def main() -> None:
    destination = (
        Path(__file__).resolve().parents[2]
        / "rust/crates/muzik-match/tests/fixtures/string_distance.json"
    )
    destination.parent.mkdir(parents=True, exist_ok=True)
    fixture = {
        "beets_version": beets.__version__,
        "cases": [
            {"left": left, "right": right, "distance": string_dist(left, right)}
            for left, right in CASES
        ],
    }
    destination.write_text(
        json.dumps(fixture, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )


if __name__ == "__main__":
    main()

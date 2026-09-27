"""Checks for the live Python to Rust album ranking seam."""

from types import SimpleNamespace

from beets.autotag.hooks import AlbumInfo, TrackInfo
from beets.library import Item

from muzik.core.matching import rank_album_candidates


def test_ranks_existing_beets_candidates_by_original_index() -> None:
    task = SimpleNamespace(
        items=[Item(title="One", artist="Band", album="Record", track=1)],
        candidates=[
            SimpleNamespace(
                info=AlbumInfo(
                    tracks=[TrackInfo(title="Other", index=1)],
                    album="Different",
                    artist="Other",
                    album_id="wrong",
                )
            ),
            SimpleNamespace(
                info=AlbumInfo(
                    tracks=[TrackInfo(title="One", index=1)],
                    album="Record",
                    artist="Band",
                    album_id="right",
                )
            ),
        ],
    )

    ranking = rank_album_candidates(task)

    assert [row.original_index for row in ranking.candidates] == [1, 0]
    assert ranking.candidates[0].distance == 0.0
    assert ranking.candidates[1].distance > 0.5
    assert ranking.recommendation == "strong"

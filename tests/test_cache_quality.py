from muzik.core.cache import (
    candidate_cache_key,
    download_cache_key,
    stable_hash,
    workflow_cache_key,
)
from muzik.core.quality import (
    is_lossless,
    normalize_format,
    parse_bitrate,
    quality_from_name,
    score_candidate,
)
from muzik.core.sources.base import Candidate, CandidateFile


def test_source_neutral_cache_keys_are_deterministic() -> None:
    left = {"source": "soulseek", "id": "peer:/file.flac"}
    right = {"id": "peer:/file.flac", "source": "soulseek"}

    assert stable_hash(left) == stable_hash(right)
    assert download_cache_key("soulseek", "peer:/file.flac").startswith(
        "download_soulseek_"
    )
    assert workflow_cache_key("soulseek", left).startswith("workflow_soulseek_")
    assert candidate_cache_key({"source": "soulseek", "path": "x"}).startswith(
        "candidate_soulseek_"
    )


def test_quality_detection_from_names_and_bitrate() -> None:
    assert normalize_format("01 Track.FLAC") == "flac"
    assert is_lossless("flac") is True
    assert is_lossless("mp3") is False
    assert parse_bitrate("Artist - Track 320kbps.mp3") == 320

    quality = quality_from_name("01 Track 320kbps.mp3")
    assert quality.format == "mp3"
    assert quality.bitrate == 320


def test_candidate_scoring_prefers_complete_lossless_album() -> None:
    lossless = Candidate(
        source="soulseek",
        source_id="lossless",
        title="Album",
        files=[
            CandidateFile(name="01 One.flac", quality=quality_from_name("01 One.flac")),
            CandidateFile(name="02 Two.flac", quality=quality_from_name("02 Two.flac")),
        ],
    )
    lossy = Candidate(
        source="soulseek",
        source_id="lossy",
        title="Album",
        files=[
            CandidateFile(
                name="01 One 128kbps.mp3",
                quality=quality_from_name("01 One 128kbps.mp3"),
            )
        ],
    )

    assert score_candidate(lossless, expected_track_count=2) > score_candidate(
        lossy,
        expected_track_count=2,
    )


def test_candidate_scoring_rewards_album_completeness_similarity_and_peer_signals() -> (
    None
):
    strong = Candidate(
        source="soulseek",
        source_id="strong",
        title="Selected Ambient Works",
        path="Aphex Twin/Selected Ambient Works",
        files=[
            CandidateFile(
                name="Aphex Twin/Selected Ambient Works/01 Xtal.flac",
                quality=quality_from_name("01 Xtal.flac"),
            ),
            CandidateFile(
                name="Aphex Twin/Selected Ambient Works/02 Tha.flac",
                quality=quality_from_name("02 Tha.flac"),
            ),
        ],
        metadata={
            "query": "Aphex Twin Selected Ambient Works flac",
            "hasFreeUploadSlot": True,
            "queueLength": 0,
            "uploadSpeed": 1_000_000,
        },
    )
    weak = Candidate(
        source="soulseek",
        source_id="weak",
        title="Random Downloads",
        path="Downloads",
        files=[
            CandidateFile(
                name="Downloads/random incomplete transcode.mp3",
                quality=quality_from_name("random 128kbps.mp3"),
            )
        ],
        metadata={
            "query": "Aphex Twin Selected Ambient Works flac",
            "hasFreeUploadSlot": False,
            "queueLength": 75,
            "uploadSpeed": 10_000,
        },
    )

    assert score_candidate(strong, expected_track_count=2) > score_candidate(
        weak,
        expected_track_count=2,
    )

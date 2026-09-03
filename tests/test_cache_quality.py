from pathlib import Path

import muzik.core.quality as quality_mod
from muzik.core.cache import (
    candidate_cache_key,
    download_cache_key,
    stable_hash,
    workflow_cache_key,
)
from muzik.core.quality import (
    QualityDecision,
    QualityPolicy,
    decide_quality,
    is_lossless,
    measure_quality,
    normalize_format,
    parse_bitrate,
    quality_from_name,
    score_candidate,
)
from muzik.core.sources.base import Candidate, CandidateFile, QualityInfo


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


def _ffprobe_payload(**stream_overrides: object) -> dict:
    stream = {
        "codec_type": "audio",
        "codec_name": "flac",
        "sample_rate": "44100",
        "channels": 2,
        "bits_per_raw_sample": "16",
        "bit_rate": "1000000",
    }
    stream.update(stream_overrides)
    return {"format": {"duration": "123.45"}, "streams": [stream]}


def test_measure_quality_reads_ffprobe_audio_stream(
    tmp_path: Path, monkeypatch
) -> None:
    audio = tmp_path / "track.flac"
    audio.write_bytes(b"01234567")
    monkeypatch.setattr(quality_mod, "probe", lambda _: _ffprobe_payload())

    quality = measure_quality(audio)

    assert quality is not None
    assert quality.format == "flac"
    assert quality.lossless is True
    assert quality.bitrate == 1000
    assert quality.sample_rate == 44100
    assert quality.bit_depth == 16
    assert quality.channels == 2
    assert quality.duration == 123.45
    assert quality.size == 8
    assert quality.measured is True


def test_measure_quality_returns_none_for_invalid_audio(
    tmp_path: Path, monkeypatch
) -> None:
    audio = tmp_path / "broken.flac"

    def failing_probe(_path: Path) -> dict:
        raise ValueError("ffprobe failed")

    monkeypatch.setattr(quality_mod, "probe", failing_probe)

    assert measure_quality(audio) is None


def test_measure_quality_returns_none_without_audio_stream(
    tmp_path: Path, monkeypatch
) -> None:
    audio = tmp_path / "video.mp4"
    audio.write_bytes(b"data")
    monkeypatch.setattr(
        quality_mod,
        "probe",
        lambda _: {
            "format": {"duration": "10"},
            "streams": [{"codec_type": "video", "codec_name": "h264"}],
        },
    )

    assert measure_quality(audio) is None


def test_measure_quality_handles_missing_fields(tmp_path: Path, monkeypatch) -> None:
    audio = tmp_path / "track.mp3"
    audio.write_bytes(b"12")
    monkeypatch.setattr(
        quality_mod,
        "probe",
        lambda _: {
            "format": {},
            "streams": [{"codec_type": "audio", "codec_name": "mp3"}],
        },
    )

    quality = measure_quality(audio)

    assert quality is not None
    assert quality.format == "mp3"
    assert quality.lossless is False
    assert quality.bitrate is None
    assert quality.sample_rate is None
    assert quality.bit_depth is None
    assert quality.channels is None
    assert quality.duration is None
    assert quality.size == 2
    assert quality.measured is True


def test_decide_quality_off_policy_always_keeps() -> None:
    lossy_quality = QualityInfo(format="mp3", lossless=False, bitrate=96)

    assert (
        decide_quality(lossy_quality, policy=QualityPolicy.OFF) == QualityDecision.KEEP
    )


def test_decide_quality_keeps_lossless_and_high_bitrate() -> None:
    lossless_quality = QualityInfo(format="flac", lossless=True, bitrate=None)
    high_bitrate_quality = QualityInfo(format="mp3", lossless=False, bitrate=320)

    assert (
        decide_quality(lossless_quality, policy=QualityPolicy.AUTO)
        == QualityDecision.KEEP
    )
    assert (
        decide_quality(high_bitrate_quality, policy=QualityPolicy.ASK, min_bitrate=256)
        == QualityDecision.KEEP
    )


def test_decide_quality_below_threshold_respects_policy() -> None:
    low_bitrate_quality = QualityInfo(format="mp3", lossless=False, bitrate=128)

    assert (
        decide_quality(low_bitrate_quality, policy=QualityPolicy.ASK, min_bitrate=256)
        == QualityDecision.ASK
    )
    assert (
        decide_quality(low_bitrate_quality, policy=QualityPolicy.AUTO, min_bitrate=256)
        == QualityDecision.REPLACE
    )

"""Audio quality detection and candidate ranking helpers."""

from __future__ import annotations

import re
from pathlib import Path
from typing import Iterable, Optional

from muzik.core.sources.base import Candidate, CandidateFile, QualityInfo


LOSSLESS_FORMATS = {"flac", "alac", "wav", "aiff", "aif", "ape", "wv"}
LOSSY_FORMATS = {"mp3", "m4a", "aac", "opus", "ogg"}
SUPPORTED_QUALITY_FORMATS = LOSSLESS_FORMATS | LOSSY_FORMATS


def normalize_format(value: str | Path | None) -> Optional[str]:
    if value is None:
        return None
    raw = str(value).lower().strip()
    if not raw:
        return None
    ext = Path(raw).suffix.lower().lstrip(".") if "." in raw else raw
    aliases = {"aif": "aiff", "m4a": "m4a", "jpeg": "jpg"}
    return aliases.get(ext, ext)


def is_lossless(format_name: str | None) -> bool:
    normalized = normalize_format(format_name)
    return bool(normalized and normalized in LOSSLESS_FORMATS)


def quality_from_name(
    name: str,
    *,
    bitrate: Optional[int] = None,
    sample_rate: Optional[int] = None,
    size: Optional[int] = None,
) -> QualityInfo:
    format_name = normalize_format(name)
    parsed_bitrate = bitrate or parse_bitrate(name)
    return QualityInfo(
        format=format_name,
        lossless=is_lossless(format_name),
        bitrate=parsed_bitrate,
        sample_rate=sample_rate,
        size=size,
    )


def parse_bitrate(value: str | None) -> Optional[int]:
    if not value:
        return None
    match = re.search(r"\b([1-9]\d{1,3})\s*kbps\b", value, flags=re.IGNORECASE)
    if match:
        return int(match.group(1))
    match = re.search(r"\b([1-9]\d{1,3})\s*k\b", value, flags=re.IGNORECASE)
    if match:
        return int(match.group(1))
    return None


def best_quality(files: Iterable[CandidateFile]) -> QualityInfo:
    best = QualityInfo()
    best_score = -1.0
    for file in files:
        score = quality_score(file.quality)
        if score > best_score:
            best = file.quality
            best_score = score
    return best


def quality_score(quality: QualityInfo, prefer: str | None = None) -> float:
    score = 0.0
    fmt = normalize_format(quality.format)
    prefer = normalize_format(prefer)

    if fmt in LOSSLESS_FORMATS:
        score += 100.0
    elif fmt == "mp3":
        score += 50.0
    elif fmt in LOSSY_FORMATS:
        score += 40.0

    if prefer:
        if prefer == "lossless" and fmt in LOSSLESS_FORMATS:
            score += 30.0
        elif prefer == "mp3-320" and fmt == "mp3" and quality.bitrate == 320:
            score += 30.0
        elif prefer == fmt:
            score += 30.0

    if quality.bitrate:
        score += min(quality.bitrate, 320) / 10
    if quality.sample_rate:
        score += min(quality.sample_rate, 192000) / 48000
    if quality.bit_depth:
        score += quality.bit_depth / 4
    return score


def score_candidate(
    candidate: Candidate,
    *,
    prefer: str | None = "lossless",
    expected_track_count: int | None = None,
    query: str | None = None,
) -> float:
    """Return a deterministic first-pass quality score for a source candidate."""
    files = candidate.files
    audio_files = [
        file
        for file in files
        if normalize_format(file.name) in SUPPORTED_QUALITY_FORMATS
        or file.quality.format in SUPPORTED_QUALITY_FORMATS
    ]

    score = quality_score(candidate.quality, prefer)
    if audio_files:
        score = max(score, quality_score(best_quality(audio_files), prefer))
        score += min(len(audio_files), 30)

    if expected_track_count:
        if len(audio_files) == expected_track_count:
            score += 25.0
        elif abs(len(audio_files) - expected_track_count) <= 1:
            score += 10.0
        elif len(audio_files) < expected_track_count:
            score -= 20.0
        elif len(audio_files) > expected_track_count * 2:
            score -= 10.0

    if audio_files:
        parents = {str(Path(file.name).parent) for file in audio_files}
        if len(parents) == 1 and "." not in parents:
            score += 8.0
        if expected_track_count and len(audio_files) >= expected_track_count:
            score += 8.0

    numbered = sum(1 for file in audio_files if re.match(r"^\D*\d{1,2}\D", file.name))
    if audio_files and numbered >= max(1, len(audio_files) // 2):
        score += 10.0

    lowered = " ".join(
        filter(
            None,
            [
                candidate.title,
                candidate.path or "",
                " ".join(f.name for f in audio_files),
            ],
        )
    ).lower()
    for bad in ("partial", "incomplete", "youtube rip", "web rip", "transcode"):
        if bad in lowered:
            score -= 15.0

    query_score = _token_overlap_score(
        query or candidate.metadata.get("query"), lowered
    )
    score += query_score

    if candidate.metadata.get("hasFreeUploadSlot"):
        score += 5.0
    queue_length = candidate.metadata.get("queueLength")
    if queue_length is not None:
        score -= min(int(queue_length), 100) / 10
    upload_speed = candidate.metadata.get("uploadSpeed")
    if upload_speed:
        score += min(int(upload_speed), 2_000_000) / 200_000

    return round(score, 3)


def _token_overlap_score(query: object, haystack: str) -> float:
    if not query:
        return 0.0
    query_tokens = {
        token
        for token in re.findall(r"[a-z0-9]+", str(query).lower())
        if len(token) > 2 and token not in {"flac", "mp3", "lossless", "the"}
    }
    if not query_tokens:
        return 0.0
    haystack_tokens = set(re.findall(r"[a-z0-9]+", haystack.lower()))
    overlap = len(query_tokens & haystack_tokens)
    return min(20.0, 20.0 * (overlap / len(query_tokens)))

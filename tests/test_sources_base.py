from pathlib import Path

from muzik.core.sources.base import (
    Candidate,
    CandidateFile,
    DownloadRequest,
    DownloadResult,
    QualityInfo,
    ResolvedRelease,
    ResolvedTrack,
)


def test_source_models_serialize_paths_and_nested_data(tmp_path: Path) -> None:
    request = DownloadRequest(raw="Artist - Album", source="soulseek")
    assert request.to_dict()["raw"] == "Artist - Album"

    track = ResolvedTrack(title="Track 1", artist="Artist", index=1)
    release = ResolvedRelease(
        title="Album",
        artist="Artist",
        tracks=[track],
        source="youtube",
        source_id="abc123",
    )
    assert release.to_dict()["tracks"][0]["title"] == "Track 1"

    candidate = Candidate(
        source="soulseek",
        source_id="peer:/music/album",
        title="Album",
        files=[
            CandidateFile(
                name="01 Track.flac",
                quality=QualityInfo(format="flac", lossless=True),
            )
        ],
    )
    assert candidate.to_dict()["files"][0]["quality"]["lossless"] is True

    audio = tmp_path / "01 Track.flac"
    result = DownloadResult(
        source="soulseek",
        source_id=candidate.source_id,
        files=[audio],
        root=tmp_path,
        metadata_path=tmp_path / ".muzik.json",
    )
    assert result.to_dict()["files"] == [str(audio)]
    assert result.to_dict()["root"] == str(tmp_path)

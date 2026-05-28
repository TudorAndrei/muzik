"""Source-neutral download models.

The command layer should speak these types instead of encoding assumptions
about YouTube, Soulseek, or any future source directly in workflow code.
"""

from __future__ import annotations

from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any, Literal, Optional, Protocol, runtime_checkable


SourceName = Literal["auto", "youtube", "soulseek", "bandcamp", "local"]


@dataclass(slots=True)
class DownloadRequest:
    raw: str
    source: SourceName = "auto"
    prefer_format: Optional[str] = None
    album: Optional[bool] = None

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


@dataclass(slots=True)
class ResolvedTrack:
    title: str
    artist: Optional[str] = None
    album: Optional[str] = None
    year: Optional[str] = None
    index: Optional[int] = None
    duration: Optional[float] = None
    source: str = "unknown"
    source_id: Optional[str] = None
    source_url: Optional[str] = None
    source_metadata: dict[str, Any] = field(default_factory=dict)

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


@dataclass(slots=True)
class ResolvedRelease:
    title: str
    artist: Optional[str] = None
    album: Optional[str] = None
    year: Optional[str] = None
    tracks: list[ResolvedTrack] = field(default_factory=list)
    source: str = "unknown"
    source_id: Optional[str] = None
    source_url: Optional[str] = None
    source_metadata: dict[str, Any] = field(default_factory=dict)

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


@dataclass(slots=True)
class ResolvedPlaylist:
    title: str
    entries: list[ResolvedRelease | ResolvedTrack] = field(default_factory=list)
    source: str = "unknown"
    source_id: Optional[str] = None
    source_url: Optional[str] = None
    source_metadata: dict[str, Any] = field(default_factory=dict)

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


@dataclass(slots=True)
class QualityInfo:
    format: Optional[str] = None
    lossless: bool = False
    bitrate: Optional[int] = None
    sample_rate: Optional[int] = None
    bit_depth: Optional[int] = None
    size: Optional[int] = None

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)

    @classmethod
    def from_dict(cls, data: dict[str, Any] | None) -> "QualityInfo":
        data = data or {}
        return cls(
            format=data.get("format"),
            lossless=bool(data.get("lossless", False)),
            bitrate=data.get("bitrate"),
            sample_rate=data.get("sample_rate"),
            bit_depth=data.get("bit_depth"),
            size=data.get("size"),
        )


@dataclass(slots=True)
class CandidateFile:
    name: str
    path: Optional[str] = None
    size: Optional[int] = None
    duration: Optional[float] = None
    quality: QualityInfo = field(default_factory=QualityInfo)

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "CandidateFile":
        return cls(
            name=str(data.get("name") or ""),
            path=data.get("path"),
            size=data.get("size"),
            duration=data.get("duration"),
            quality=QualityInfo.from_dict(data.get("quality")),
        )


@dataclass(slots=True)
class Candidate:
    source: str
    source_id: str
    title: str
    user: Optional[str] = None
    path: Optional[str] = None
    files: list[CandidateFile] = field(default_factory=list)
    quality: QualityInfo = field(default_factory=QualityInfo)
    score: float = 0.0
    metadata: dict[str, Any] = field(default_factory=dict)

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "Candidate":
        metadata = data.get("metadata")
        if not isinstance(metadata, dict):
            metadata = {}
        return cls(
            source=str(data.get("source") or "unknown"),
            source_id=str(data.get("source_id") or ""),
            title=str(data.get("title") or ""),
            user=data.get("user"),
            path=data.get("path"),
            files=[
                CandidateFile.from_dict(file)
                for file in data.get("files", [])
                if isinstance(file, dict)
            ],
            quality=QualityInfo.from_dict(data.get("quality")),
            score=float(data.get("score") or 0),
            metadata=metadata,
        )


@dataclass(slots=True)
class DownloadResult:
    source: str
    source_id: str
    files: list[Path]
    root: Path
    metadata_path: Optional[Path] = None
    metadata: dict[str, Any] = field(default_factory=dict)

    def to_dict(self) -> dict[str, Any]:
        data = asdict(self)
        data["files"] = [str(path) for path in self.files]
        data["root"] = str(self.root)
        data["metadata_path"] = str(self.metadata_path) if self.metadata_path else None
        return data


@runtime_checkable
class Source(Protocol):
    name: str

    def resolve(
        self, request: DownloadRequest
    ) -> ResolvedRelease | ResolvedPlaylist | ResolvedTrack: ...

    def search(self, resolved: ResolvedRelease | ResolvedTrack) -> list[Candidate]: ...

    def download(self, candidate: Candidate, output: Path) -> DownloadResult: ...

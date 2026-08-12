"""Configuration collected by an interactive workflow launcher."""

from dataclasses import dataclass
from pathlib import Path

from muzik.config import DEFAULT_DOWNLOAD_DIR, DEFAULT_SPLITS_DIR
from muzik.core.workflow.service import AudioFallback, AudioSource, MetadataSource


@dataclass(frozen=True, slots=True)
class WorkflowLaunchConfig:
    """Values needed to start one workflow run."""

    raw: str
    output: Path = DEFAULT_DOWNLOAD_DIR
    splits: Path = DEFAULT_SPLITS_DIR
    review: bool = False
    no_split: bool = False
    no_organize: bool = False
    import_: bool = False
    tag_only: bool = False
    dry_run: bool = False
    jobs: int = 0
    config: Path | None = None
    keep_source: bool = False
    force: bool = False
    metadata_source: MetadataSource = MetadataSource.AUTO
    audio_source: AudioSource = AudioSource.YOUTUBE
    prefer: str = "lossless"
    fallback: AudioFallback = AudioFallback.YOUTUBE
    interactive: bool = True

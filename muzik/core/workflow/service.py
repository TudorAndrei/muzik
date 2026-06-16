"""UI-neutral workflow service helpers."""

from __future__ import annotations

from collections.abc import Callable
from datetime import datetime
import os
from dataclasses import dataclass
from pathlib import Path
from typing import Protocol, cast

from muzik.config import AUDIO_EXTENSIONS
from muzik.core.audio import get_duration
import muzik.core.cache as cache_mod
from muzik.core.chapters import Chapter
from muzik.core.sources.base import (
    Candidate,
    DownloadRequest,
    DownloadResult,
    ResolvedRelease,
    ResolvedTrack,
)
from muzik.core.sources.soulseek import SoulseekError, SoulseekSource
from muzik.core.sources.youtube import (
    YouTubeSource,
    find_audio_by_id,
    playlist_id as parse_playlist_id,
    youtube_id as parse_youtube_id,
)
from muzik.core.workflow.decisions import WorkflowDecisionError, WorkflowDecisions
from muzik.core.workflow.events import (
    CandidatesFoundEvent,
    MessageEvent,
    NullWorkflowEventEmitter,
    StepFinishedEvent,
    StepStartedEvent,
    WorkflowEventEmitter,
)


@dataclass(frozen=True, slots=True)
class WorkflowRequest:
    raw: str
    output: Path
    splits: Path


@dataclass(frozen=True, slots=True)
class WorkflowOptions:
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
    metadata_source: str = "auto"
    audio_source: str = "youtube"
    prefer: str = "lossless"
    fallback: str = "youtube"
    interactive: bool = True


class WorkflowServiceError(RuntimeError):
    def __init__(
        self,
        message: str,
        *,
        exit_code: int = 1,
        warnings: list[str] | None = None,
    ) -> None:
        super().__init__(message)
        self.message = message
        self.exit_code = exit_code
        self.warnings = warnings or []


@dataclass(frozen=True, slots=True)
class AudioProcessingPlan:
    albums: list[tuple[Path, list[Chapter]]]
    singles: list[Path]
    pre_split_dirs: list[Path]

    @property
    def split_dirs(self) -> list[Path]:
        return list(self.pre_split_dirs)


@dataclass(frozen=True, slots=True)
class AudioProcessingResult:
    plan: AudioProcessingPlan
    split_dirs: list[Path]
    organize_targets: list[Path]


@dataclass(frozen=True, slots=True)
class SplitTask:
    source: Path
    chapters: list[Chapter]
    output: Path


@dataclass(frozen=True, slots=True)
class WorkflowRunOperations:
    download_audio: Callable[[str, Path, Path | None], bool]
    process_audio: Callable[[list[Path], list[Path]], None]
    acquire_soulseek: Callable[[str], list[Path]]
    prepopulate_archive: Callable[[Path], None]
    get_playlist_video_ids: Callable[[str], list[str]]


class SoulseekWorkflowSource(Protocol):
    def resolve(self, request: DownloadRequest) -> ResolvedRelease | ResolvedTrack: ...

    def search(
        self,
        resolved: ResolvedRelease | ResolvedTrack,
        *,
        prefer: str,
        limit: int,
    ) -> list[Candidate]: ...

    def download(self, candidate: Candidate, wait: bool) -> DownloadResult: ...


class MetadataWorkflowSource(Protocol):
    def resolve(self, request: DownloadRequest) -> object: ...


class AudioProcessingHooks(Protocol):
    def albums_detected(self, albums: list[tuple[Path, list[Chapter]]]) -> None: ...

    def singles_detected(self, singles: list[Path]) -> None: ...

    def split_started(self, task: SplitTask, *, dry_run: bool) -> None: ...

    def split_failed(self, source: Path) -> None: ...

    def organize_started(self, target: Path) -> None: ...

    def complete(self, *, organized: bool) -> None: ...


class NullAudioProcessingHooks:
    def albums_detected(self, albums: list[tuple[Path, list[Chapter]]]) -> None:
        return None

    def singles_detected(self, singles: list[Path]) -> None:
        return None

    def split_started(self, task: SplitTask, *, dry_run: bool) -> None:
        return None

    def split_failed(self, source: Path) -> None:
        return None

    def organize_started(self, target: Path) -> None:
        return None

    def complete(self, *, organized: bool) -> None:
        return None


def _default_soulseek_source() -> SoulseekWorkflowSource:
    return cast(SoulseekWorkflowSource, SoulseekSource())


def _default_youtube_source() -> MetadataWorkflowSource:
    return cast(MetadataWorkflowSource, YouTubeSource())


def load_playlist_state(playlist_id: str) -> dict:
    state = cache_mod.get_json(f"playlist_{playlist_id}") or {}
    state.setdefault("playlist_id", playlist_id)
    state.setdefault("videos", {})
    return state


def save_playlist_state(playlist_id: str, state: dict) -> None:
    state["last_updated"] = datetime.now().isoformat(timespec="seconds")
    cache_mod.set_json(f"playlist_{playlist_id}", state)


def backfill_playlist_entry_from_legacy_cache(
    video_id: str,
    *,
    splits: Path,
) -> dict:
    legacy_path = cache_mod.get(f"yt_{video_id}")
    if not legacy_path:
        return {}

    audio_path = Path(legacy_path.strip())
    if audio_path.exists():
        return {
            "status": "downloaded",
            "audio_file": legacy_path,
        }

    expected_split = splits / audio_path.stem
    if expected_split.exists():
        return {
            "status": "split",
            "audio_file": legacy_path,
            "split_dir": str(expected_split.resolve()),
        }

    return {
        "status": "organized",
        "audio_file": legacy_path,
    }


def find_audio_inputs(paths: list[Path]) -> list[Path]:
    """Return supported audio files from a mix of files and directories."""
    audio_files: list[Path] = []
    seen: set[Path] = set()
    for path in paths:
        if not path.exists():
            continue
        candidates = [path]
        if path.is_dir():
            candidates = sorted(path.rglob("*"))
        for candidate in candidates:
            if candidate.is_file() and candidate.suffix.lower() in AUDIO_EXTENSIONS:
                resolved = candidate.resolve()
                if resolved not in seen:
                    audio_files.append(candidate)
                    seen.add(resolved)
    return sorted(audio_files)


def common_parent(paths: list[Path]) -> Path | None:
    if not paths:
        return None
    if len(paths) == 1:
        return paths[0]
    try:
        return Path(os.path.commonpath([str(path.parent) for path in paths]))
    except ValueError:
        return None


def plan_audio_processing(
    audio_files: list[Path],
    *,
    pre_split_dirs: list[Path],
    chapter_resolver: Callable[[Path], list[Chapter] | None],
) -> AudioProcessingPlan:
    albums: list[tuple[Path, list[Chapter]]] = []
    singles: list[Path] = []

    for audio_file in audio_files:
        chapters = chapter_resolver(audio_file)
        if chapters:
            albums.append((audio_file, chapters))
        else:
            singles.append(audio_file)

    return AudioProcessingPlan(
        albums=albums,
        singles=singles,
        pre_split_dirs=list(pre_split_dirs),
    )


def organize_targets_for_singles(singles: list[Path]) -> list[Path]:
    single_root = common_parent(singles)
    if single_root and single_root.is_dir() and len(singles) > 1:
        return [single_root]
    return singles


def process_audio_plan(
    *,
    audio_files: list[Path],
    pre_split_dirs: list[Path],
    splits: Path,
    options: WorkflowOptions,
    chapter_resolver: Callable[[Path], list[Chapter] | None],
    split_operation: Callable[[SplitTask], bool],
    organize_operation: Callable[[Path], bool],
    events: WorkflowEventEmitter | None = None,
    hooks: AudioProcessingHooks | None = None,
) -> AudioProcessingResult:
    events = events or NullWorkflowEventEmitter()
    hooks = hooks or NullAudioProcessingHooks()
    plan = plan_audio_processing(
        audio_files,
        pre_split_dirs=pre_split_dirs,
        chapter_resolver=chapter_resolver,
    )

    if plan.albums:
        hooks.albums_detected(plan.albums)
    if plan.singles:
        hooks.singles_detected(plan.singles)

    split_dirs = plan.split_dirs
    if plan.albums:
        events.emit(
            StepStartedEvent(name="split", detail=f"{len(plan.albums)} album(s)")
        )
        for source, chapters in plan.albums:
            task = SplitTask(
                source=source,
                chapters=chapters,
                output=splits / source.stem,
            )
            hooks.split_started(task, dry_run=options.dry_run)
            if options.dry_run:
                continue
            if split_operation(task):
                split_dirs.append(task.output)
            else:
                hooks.split_failed(source)
        events.emit(
            StepFinishedEvent(
                name="split",
                detail=f"{len(split_dirs)} output dir(s)",
            )
        )

    if options.no_organize:
        hooks.complete(organized=False)
        return AudioProcessingResult(
            plan=plan,
            split_dirs=split_dirs,
            organize_targets=[],
        )

    events.emit(StepStartedEvent(name="organize"))
    organize_targets = [*split_dirs, *organize_targets_for_singles(plan.singles)]
    for target in organize_targets:
        hooks.organize_started(target)
        if not options.dry_run:
            organize_operation(target)
    events.emit(StepFinishedEvent(name="organize"))
    hooks.complete(organized=True)

    return AudioProcessingResult(
        plan=plan,
        split_dirs=split_dirs,
        organize_targets=organize_targets,
    )


def validated_audio_files(
    audio_files: list[Path],
    *,
    dry_run: bool,
    no_organize: bool,
    duration_probe: Callable[[Path], float | None] | None = None,
) -> tuple[list[Path], list[str]]:
    """Return plausible audio files and warning messages for rejected paths."""
    duration_probe = duration_probe or get_duration
    if dry_run or no_organize:
        return audio_files, []

    valid: list[Path] = []
    warnings: list[str] = []
    for path in audio_files:
        if not path.exists():
            warnings.append(f"Skipping missing audio file: {path}")
            continue
        if path.suffix.lower() not in AUDIO_EXTENSIONS:
            warnings.append(f"Skipping unsupported audio file: {path}")
            continue
        duration = duration_probe(path)
        if duration is None or duration <= 0:
            warnings.append(f"Skipping unprobeable audio file: {path}")
            continue
        valid.append(path)

    if audio_files and not valid:
        raise WorkflowServiceError(
            "No downloaded audio files passed validation.",
            warnings=warnings,
        )
    return valid, warnings


def resolve_soulseek_request(
    request: str,
    *,
    prefer: str,
    source: SoulseekWorkflowSource,
    youtube_source: MetadataWorkflowSource | None = None,
) -> ResolvedRelease | ResolvedTrack:
    """Resolve user input into metadata suitable for Soulseek search."""
    if parse_youtube_id(request):
        metadata_source = youtube_source or _default_youtube_source()
        resolved = metadata_source.resolve(
            DownloadRequest(raw=request, source="youtube")
        )
        if not isinstance(resolved, (ResolvedRelease, ResolvedTrack)):
            raise WorkflowServiceError(
                "Expected a single YouTube video, got a playlist"
            )
        return resolved

    return source.resolve(
        DownloadRequest(
            raw=request,
            source="soulseek",
            prefer_format=prefer,
            album=True,
        )
    )


def record_soulseek_download(request: str, result: DownloadResult) -> None:
    cache_mod.set_json(
        cache_mod.workflow_cache_key("soulseek", request),
        {
            "status": "downloaded",
            "source": "soulseek",
            "source_id": result.source_id,
            "files": [str(path.resolve()) for path in result.files],
            "metadata_path": (
                str(result.metadata_path.resolve()) if result.metadata_path else None
            ),
        },
    )


def acquire_from_soulseek(
    request: str,
    *,
    prefer: str,
    fallback: str,
    decisions: WorkflowDecisions,
    events: WorkflowEventEmitter | None = None,
    source_factory: Callable[[], SoulseekWorkflowSource] = _default_soulseek_source,
    youtube_source_factory: Callable[
        [],
        MetadataWorkflowSource,
    ] = _default_youtube_source,
) -> list[Path]:
    """Search/download audio through Soulseek and return local audio paths."""
    events = events or NullWorkflowEventEmitter()
    source = source_factory()
    try:
        resolved = resolve_soulseek_request(
            request,
            prefer=prefer,
            source=source,
            youtube_source=youtube_source_factory(),
        )
        candidates = source.search(resolved, prefer=prefer, limit=10)
        events.emit(
            CandidatesFoundEvent(
                candidates=candidates,
                source="soulseek",
                limit=10,
            )
        )
    except Exception as exc:
        if fallback == "youtube" and parse_youtube_id(request):
            return []
        if isinstance(exc, WorkflowServiceError):
            raise
        raise WorkflowServiceError(f"Soulseek search failed: {exc}") from exc

    if not candidates:
        if fallback == "youtube" and parse_youtube_id(request):
            return []
        raise WorkflowServiceError("No Soulseek candidates found.", exit_code=0)

    try:
        candidate = decisions.choose_soulseek_candidate(candidates)
    except WorkflowDecisionError as exc:
        raise WorkflowServiceError(str(exc)) from exc

    events.emit(
        MessageEvent(
            f"Selected Soulseek candidate: {candidate.title or candidate.source_id}"
        )
    )
    events.emit(MessageEvent("Downloading selected Soulseek candidate."))
    try:
        result = source.download(candidate, wait=True)
    except SoulseekError as exc:
        raise WorkflowServiceError(f"Soulseek download failed: {exc}") from exc

    events.emit(
        MessageEvent(f"Soulseek download returned {len(result.files)} file(s).")
    )
    record_soulseek_download(request, result)
    if not result.files:
        raise WorkflowServiceError(
            "Soulseek download was enqueued, but no local audio files were found. "
            "Check SLSKD_DOWNLOAD_DIR.",
            exit_code=0,
        )
    return result.files


def _new_audio_files(before: set[Path], after: set[Path]) -> list[Path]:
    return sorted(
        path
        for path in (after - before)
        if path.is_file() and path.suffix.lower() in AUDIO_EXTENSIONS
    )


def _existing_cached_audio(cache_key: str | None) -> Path | None:
    cached_entry = cache_mod.get(cache_key) if cache_key else None
    if not cached_entry:
        return None
    cached_path = Path(cached_entry.strip())
    return cached_path if cached_path.exists() else None


def run_workflow(
    request: WorkflowRequest,
    options: WorkflowOptions,
    *,
    operations: WorkflowRunOperations,
    events: WorkflowEventEmitter | None = None,
) -> None:
    """Run the top-level workflow using injected UI/tool operations."""
    events = events or NullWorkflowEventEmitter()
    events.emit(StepStartedEvent(name="download", detail=request.raw))

    yt_id = parse_youtube_id(request.raw)
    playlist_id = parse_playlist_id(request.raw)

    if playlist_id:
        _run_playlist_workflow(
            request,
            options,
            playlist_id=playlist_id,
            operations=operations,
            events=events,
        )
        events.emit(StepFinishedEvent(name="download", detail=request.raw))
        return

    audio_files, pre_split_dirs = _acquire_single_workflow_inputs(
        request,
        options,
        yt_id=yt_id,
        operations=operations,
    )
    events.emit(StepFinishedEvent(name="download", detail=request.raw))
    operations.process_audio(audio_files, pre_split_dirs)


def _run_playlist_workflow(
    request: WorkflowRequest,
    options: WorkflowOptions,
    *,
    playlist_id: str,
    operations: WorkflowRunOperations,
    events: WorkflowEventEmitter,
) -> None:
    archive_file = cache_mod.CACHE_DIR / f"ytdlp_archive_{playlist_id}.txt"
    operations.prepopulate_archive(archive_file)
    playlist_state = load_playlist_state(playlist_id)

    if options.dry_run:
        return

    video_ids = operations.get_playlist_video_ids(request.raw)
    if not video_ids:
        raise WorkflowServiceError(
            "Could not fetch playlist video IDs — check the URL and yt-dlp."
        )

    for video_id in video_ids:
        _process_playlist_video(
            video_id,
            playlist_id=playlist_id,
            playlist_state=playlist_state,
            request=request,
            options=options,
            archive_file=archive_file,
            operations=operations,
            events=events,
        )


def _process_playlist_video(
    video_id: str,
    *,
    playlist_id: str,
    playlist_state: dict,
    request: WorkflowRequest,
    options: WorkflowOptions,
    archive_file: Path,
    operations: WorkflowRunOperations,
    events: WorkflowEventEmitter,
) -> None:
    entry = playlist_state["videos"].get(video_id, {})
    if options.force and entry.get("status") in ("split", "organized"):
        entry = {key: value for key, value in entry.items() if key != "status"}
        entry["status"] = "downloaded"

    if not entry:
        entry = backfill_playlist_entry_from_legacy_cache(
            video_id,
            splits=request.splits,
        )

    if entry.get("status") == "organized":
        return

    video_url = f"https://www.youtube.com/watch?v={video_id}"
    if options.audio_source == "soulseek":
        files_for_video = _cached_playlist_files(entry)
        if not files_for_video:
            try:
                files_for_video = operations.acquire_soulseek(video_url)
            except WorkflowServiceError:
                return
            playlist_state["videos"][video_id] = {
                "status": "downloaded",
                "source": "soulseek",
                "files": [str(path.resolve()) for path in files_for_video],
            }
            save_playlist_state(playlist_id, playlist_state)

        operations.process_audio(files_for_video, [])
        if not options.no_organize:
            playlist_state["videos"][video_id]["status"] = "organized"
            save_playlist_state(playlist_id, playlist_state)
        return

    split_dir_for_video: Path | None = None
    audio_file: Path | None = None

    if entry.get("status") == "split":
        split_dir = Path(entry.get("split_dir", ""))
        if split_dir.exists():
            split_dir_for_video = split_dir

    if split_dir_for_video is None:
        if entry.get("status") == "downloaded":
            cached = Path(entry["audio_file"])
            if cached.exists():
                audio_file = cached

        if audio_file is None:
            before = set(request.output.glob("*")) if request.output.exists() else set()
            if not operations.download_audio(video_url, request.output, archive_file):
                return
            after = set(request.output.glob("*")) if request.output.exists() else set()
            new_files = _new_audio_files(before, after)
            if not new_files:
                new_files = find_audio_by_id(request.output, video_id)
            if not new_files:
                return
            audio_file = new_files[0]
            playlist_state["videos"][video_id] = {
                "status": "downloaded",
                "audio_file": str(audio_file.resolve()),
            }
            save_playlist_state(playlist_id, playlist_state)
            cache_mod.set(f"yt_{video_id}", str(audio_file.resolve()))

        operations.process_audio([audio_file], [])
        if not options.no_organize:
            playlist_state["videos"][video_id]["status"] = "organized"
            save_playlist_state(playlist_id, playlist_state)
        return

    if split_dir_for_video is not None and not options.no_organize:
        operations.process_audio([], [split_dir_for_video])
        playlist_state["videos"][video_id]["status"] = "organized"
        save_playlist_state(playlist_id, playlist_state)


def _cached_playlist_files(entry: dict) -> list[Path]:
    if entry.get("status") != "downloaded":
        return []
    return [Path(file) for file in entry.get("files") or [] if Path(file).exists()]


def _acquire_single_workflow_inputs(
    request: WorkflowRequest,
    options: WorkflowOptions,
    *,
    yt_id: str | None,
    operations: WorkflowRunOperations,
) -> tuple[list[Path], list[Path]]:
    audio_files: list[Path] = []
    pre_split_dirs: list[Path] = []

    if options.dry_run:
        return audio_files, pre_split_dirs

    local_path = Path(request.raw).expanduser()
    local_input = local_path.exists()
    if local_input:
        audio_files = find_audio_inputs([local_path])

    if not local_input and options.audio_source == "soulseek":
        audio_files = operations.acquire_soulseek(request.raw)

    cache_key = f"yt_{yt_id}" if yt_id else None
    cached_path = (
        _existing_cached_audio(cache_key)
        if not audio_files and not local_input
        else None
    )
    if cached_path:
        return [cached_path], pre_split_dirs

    cached_entry = (
        cache_mod.get(cache_key)
        if cache_key and not audio_files and not local_input
        else None
    )
    if cached_entry:
        missing_cached_path = Path(cached_entry.strip())
        if not options.force:
            expected_split = request.splits / missing_cached_path.stem
            if expected_split.exists():
                pre_split_dirs.append(expected_split)
        return audio_files, pre_split_dirs

    if local_input:
        return audio_files, pre_split_dirs

    if not audio_files and yt_id and request.output.exists():
        audio_files = find_audio_by_id(request.output, yt_id)
    if audio_files:
        if cache_key:
            cache_mod.set(cache_key, str(audio_files[0]))
        return audio_files, pre_split_dirs

    before = set(request.output.glob("*")) if request.output.exists() else set()
    if not operations.download_audio(request.raw, request.output, None):
        raise WorkflowServiceError("Download failed. Aborting workflow.")
    after = set(request.output.glob("*")) if request.output.exists() else set()
    audio_files = _new_audio_files(before, after)
    if not audio_files and yt_id and request.output.exists():
        audio_files = find_audio_by_id(request.output, yt_id)
    if audio_files and cache_key:
        cache_mod.set(cache_key, str(audio_files[0]))
    return audio_files, pre_split_dirs

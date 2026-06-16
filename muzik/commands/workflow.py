"""music workflow <url> — full pipeline: download → split → organize.

Handles two scenarios automatically:
  • Single track  — no chapter markers → goes straight to beets
  • Album/chapters — has chapter markers → split first, then beets

For playlist URLs the pipeline runs per-video: each track is downloaded,
classified, split (if needed), and organized before the next one starts,
so you can review/edit chapters as they arrive.
"""

from pathlib import Path
from typing import Optional, cast

import typer

from muzik.commands.download import download_cmd
from muzik.commands.split import split_cmd
from muzik.commands.organize import organize_cmd
from muzik.config import (
    BEETS_CONFIG,
    DEFAULT_DOWNLOAD_DIR,
    DEFAULT_SPLITS_DIR,
)
from muzik.core.audio import extract_metadata, get_duration
from muzik.core.chapters import Chapter, find_chapters, serialize_chapters
from muzik.core.musicbrainz import (
    MIN_ALBUM_DURATION,
    lookup_chapters_verbose as lookup_chapters,
)
from muzik.core.description_chapters import (
    description_has_timestamps,
    extract_chapters_from_description,
    get_description_from_info_json,
)
from muzik.core.sources.soulseek import SoulseekSource
from muzik.core.sources.youtube import (
    get_playlist_video_ids,
    prepopulate_archive,
    youtube_id as parse_youtube_id,
)
from muzik.core.workflow.decisions import (
    ChapterDecision,
    WorkflowDecisions,
)
from muzik.core.workflow.events import (
    ChapterReviewRequestedEvent,
    NullWorkflowEventEmitter,
    WorkflowEventEmitter,
)
from muzik.core.workflow.service import (
    WorkflowOptions,
    WorkflowRequest,
    WorkflowRunOperations,
    WorkflowServiceError,
    SplitTask,
    MetadataWorkflowSource,
    SoulseekWorkflowSource,
    acquire_from_soulseek as acquire_soulseek_audio,
    common_parent as _common_parent,
    find_audio_inputs as _find_audio_inputs,
    process_audio_plan,
    run_workflow,
    validated_audio_files,
)
from muzik.core.sources.youtube import YouTubeSource
from muzik.ui.cli.decisions import CliWorkflowDecisions
from muzik.ui.chapter_editor import display_chapter_table
from muzik.ui.console import console, err


def _youtube_id(url: str) -> Optional[str]:
    """Extract the 11-char YouTube video ID from a URL, or None for playlists."""
    return parse_youtube_id(url)


def _prepopulate_archive(archive_file: Path) -> None:
    """Seed the yt-dlp archive with every individually-cached video ID.

    Scans all ``yt_<id>.txt`` files in the cache dir and appends any IDs not
    already present in the archive, so yt-dlp skips re-downloading songs that
    were previously processed as standalone ``muzik workflow <video-url>`` runs.
    """
    prepopulate_archive(archive_file)


def _get_playlist_video_ids(url: str) -> list[str]:
    """Return ordered video IDs in a YouTube playlist via yt-dlp."""
    return get_playlist_video_ids(url)


def _soulseek_source() -> SoulseekWorkflowSource:
    return cast(SoulseekWorkflowSource, SoulseekSource())


def _youtube_source() -> MetadataWorkflowSource:
    return cast(MetadataWorkflowSource, YouTubeSource())


def _get_chapters_for(
    af: Path,
    no_split: bool,
    decisions: WorkflowDecisions | None = None,
    events: WorkflowEventEmitter | None = None,
) -> Optional[list]:
    """Return chapter list if *af* should be split as an album, else ``None``.

    Checks embedded chapters first; if none found and the file looks long enough
    to be an album, falls back to an interactive MusicBrainz lookup.
    Returns ``None`` immediately when *no_split* is set.
    """
    decisions = decisions or CliWorkflowDecisions()
    events = events or NullWorkflowEventEmitter()
    if no_split:
        return None
    chapters = find_chapters(af)
    if chapters:
        return chapters
    duration = get_duration(af)
    if not (duration and duration >= MIN_ALBUM_DURATION):
        return None
    meta = extract_metadata(af)
    artist = meta.get("artist", "")
    album_name = meta.get("album", "")
    year = meta.get("year", "")
    console.print(
        f"\n  [yellow]No chapters:[/yellow] {af.name} "
        f"([dim]{int(duration) // 60}m, looks like an album[/dim])"
    )
    console.print(f"  [dim]Querying MusicBrainz for {artist!r} / {album_name!r}…[/dim]")
    mb_chapters, mb_title, mb_diag = lookup_chapters(artist, album_name, year)
    if not mb_chapters:
        console.print("  [dim]MusicBrainz: no match found.[/dim]")
        for line in mb_diag.splitlines():
            console.print(f"  [dim]  {line}[/dim]")
        # Try extracting chapters from the video description via LLM
        import os

        jsn = af.with_suffix("").with_suffix(".info.json")
        if jsn.exists() and os.environ.get("OPENROUTER_API_KEY"):
            description = get_description_from_info_json(jsn)
            if description and description_has_timestamps(description):
                console.print(
                    "  [dim]Querying LLM for chapters in video description…[/dim]"
                )
                llm_chapters, llm_err = extract_chapters_from_description(description)
                if llm_err:
                    console.print(f"  [red]LLM error:[/red] {llm_err}")
                elif llm_chapters:
                    console.print(
                        f"  [cyan]LLM found:[/cyan] {len(llm_chapters)} tracks in description"
                    )
                    events.emit(
                        ChapterReviewRequestedEvent(
                            source=af,
                            chapters=llm_chapters,
                            title="LLM — description",
                        )
                    )
                    display_chapter_table(llm_chapters, title="LLM — description")
                    chapter_decision = decisions.confirm_chapters(af, llm_chapters)
                    if chapter_decision == ChapterDecision.EDIT:
                        edited = decisions.edit_chapters(llm_chapters)
                        if edited is None:
                            chapter_decision = ChapterDecision.REJECT
                        else:
                            llm_chapters = edited
                            chapter_decision = ChapterDecision.ACCEPT
                    if chapter_decision == ChapterDecision.ACCEPT:
                        sidecar = af.with_suffix(".chapters.txt")
                        sidecar.write_text(
                            serialize_chapters(llm_chapters), encoding="utf-8"
                        )
                        console.print(f"  [green]Saved:[/green] {sidecar.name}")
                        return llm_chapters
                    console.print("  [dim]Skipping LLM chapters.[/dim]")
                else:
                    console.print(
                        "  [dim]LLM: no timestamps found in description.[/dim]"
                    )
        return None
    console.print(
        f"  [cyan]MusicBrainz found:[/cyan] [bold]{mb_title}[/bold] — {len(mb_chapters)} tracks"
    )
    events.emit(
        ChapterReviewRequestedEvent(
            source=af,
            chapters=mb_chapters,
            title=f"MusicBrainz — {mb_title}",
        )
    )
    display_chapter_table(mb_chapters, title=f"MusicBrainz — {mb_title}")
    chapter_decision = decisions.confirm_chapters(af, mb_chapters)
    if chapter_decision == ChapterDecision.EDIT:
        edited = decisions.edit_chapters(mb_chapters)
        if edited is None:
            chapter_decision = ChapterDecision.REJECT
        else:
            mb_chapters = edited
            chapter_decision = ChapterDecision.ACCEPT
    if chapter_decision != ChapterDecision.ACCEPT:
        console.print("  [dim]Skipping MusicBrainz chapters.[/dim]")
        return None
    sidecar = af.with_suffix(".chapters.txt")
    sidecar.write_text(serialize_chapters(mb_chapters), encoding="utf-8")
    console.print(f"  [green]Saved:[/green] {sidecar.name}")
    return mb_chapters


def _validated_audio_files(
    audio_files: list[Path],
    *,
    dry_run: bool,
    no_organize: bool,
) -> list[Path]:
    """Return files that are plausible enough to pass to split/organize."""
    try:
        valid, warnings = validated_audio_files(
            audio_files,
            dry_run=dry_run,
            no_organize=no_organize,
            duration_probe=get_duration,
        )
    except WorkflowServiceError as exc:
        for warning in exc.warnings:
            err(f"  [red]{warning}[/red]")
        err(f"[red]{exc.message}[/red]")
        raise typer.Exit(exc.exit_code) from exc

    for warning in warnings:
        err(f"  [red]{warning}[/red]")
    return valid


class _CliAudioProcessingHooks:
    def __init__(self) -> None:
        self._split_header_printed = False
        self._organize_header_printed = False

    def albums_detected(self, albums: list[tuple[Path, list[Chapter]]]) -> None:
        console.print(
            f"\n  [cyan]Album(s) detected:[/cyan] "
            f"{len(albums)} file(s) with chapter markers"
        )

    def singles_detected(self, singles: list[Path]) -> None:
        console.print(
            f"\n  [yellow]Single track(s) detected:[/yellow] "
            f"{len(singles)} file(s) without chapters"
        )
        single_root = _common_parent(singles)
        if single_root and single_root.is_dir() and len(singles) > 1:
            console.print(
                f"  [yellow]{len(singles)} track files[/yellow] — "
                f"will organize directory {single_root}"
            )
        else:
            for af in singles:
                console.print(
                    f"  [yellow]{af.name}[/yellow] — no chapters, will organize directly"
                )

    def split_started(self, task: SplitTask, *, dry_run: bool) -> None:
        if not self._split_header_printed:
            console.print("\n[bold]Step 2a — Split (album)[/bold]")
            self._split_header_printed = True
        console.print(
            f"  [cyan]{task.source.name}[/cyan] — {len(task.chapters)} chapters"
        )
        if dry_run:
            console.print(f"  [dim]Would split → {task.output}[/dim]")

    def split_failed(self, source: Path) -> None:
        err(f"  [red]Split failed for {source.name}[/red]")

    def organize_started(self, target: Path) -> None:
        if not self._organize_header_printed:
            console.print("\n[bold]Step 3 — Organize[/bold]")
            self._organize_header_printed = True
        console.print(f"  beet import [dim]{target}[/dim]")

    def complete(self, *, organized: bool) -> None:
        console.rule()
        if organized:
            console.print("[bold green]Workflow complete.[/bold green]")
        else:
            console.print(
                "[bold green]Workflow complete (organize skipped).[/bold green]"
            )


def _process_audio_files(
    *,
    audio_inputs: list[Path],
    pre_split_dirs: list[Path],
    splits: Path,
    review: bool,
    no_split: bool,
    no_organize: bool,
    import_: bool,
    tag_only: bool,
    dry_run: bool,
    jobs: int,
    config: Optional[Path],
    keep_source: bool,
    force: bool,
    decisions: WorkflowDecisions | None = None,
    events: WorkflowEventEmitter | None = None,
) -> None:
    """Classify, split, and organize local audio files/directories."""
    audio_files = _find_audio_inputs(audio_inputs)
    audio_files = _validated_audio_files(
        audio_files,
        dry_run=dry_run,
        no_organize=no_organize,
    )
    if not audio_files and not pre_split_dirs and not dry_run:
        err("[yellow]No audio files found in output directory.[/yellow]")
        raise typer.Exit(0)

    decisions = decisions or CliWorkflowDecisions()
    events = events or NullWorkflowEventEmitter()
    options = WorkflowOptions(
        review=review,
        no_split=no_split,
        no_organize=no_organize,
        import_=import_,
        tag_only=tag_only,
        dry_run=dry_run,
        jobs=jobs,
        config=config,
        keep_source=keep_source,
        force=force,
    )

    def split_operation(task: SplitTask) -> bool:
        try:
            split_cmd(
                path=task.source,
                review=review,
                jobs=jobs,
                output=task.output,
                keep_source=keep_source,
                force=force,
            )
        except (SystemExit, typer.Exit) as exc:
            return getattr(exc, "code", 0) == 0
        return task.output.exists()

    def organize_operation(target: Path) -> bool:
        try:
            organize_cmd(
                directory=target,
                import_=import_,
                tag_only=tag_only,
                dry_run=dry_run,
                config=config,
            )
        except (SystemExit, typer.Exit) as exc:
            if getattr(exc, "code", 0) != 0:
                err(f"  [red]beet failed for {target}[/red]")
                return False
        return True

    process_audio_plan(
        audio_files=audio_files,
        pre_split_dirs=pre_split_dirs,
        splits=splits,
        options=options,
        chapter_resolver=lambda af: _get_chapters_for(
            af,
            no_split,
            decisions,
            events,
        ),
        split_operation=split_operation,
        organize_operation=organize_operation,
        events=events,
        hooks=_CliAudioProcessingHooks(),
    )


def _acquire_from_soulseek(
    request: str,
    *,
    prefer: str,
    interactive: bool,
    fallback: str,
    decisions: WorkflowDecisions | None = None,
    events: WorkflowEventEmitter | None = None,
) -> list[Path]:
    """Search/download audio through Soulseek and return local audio paths.

    When *request* is a YouTube URL, yt-dlp is used only to resolve metadata; the
    actual audio still comes from Soulseek.
    """
    events = events or NullWorkflowEventEmitter()
    decisions = decisions or CliWorkflowDecisions(interactive=interactive)
    if _youtube_id(request):
        console.print("  [dim]Resolving YouTube metadata for Soulseek search…[/dim]")
    try:
        return acquire_soulseek_audio(
            request,
            prefer=prefer,
            fallback=fallback,
            decisions=decisions,
            events=events,
            source_factory=_soulseek_source,
            youtube_source_factory=_youtube_source,
        )
    except WorkflowServiceError as exc:
        if exc.exit_code == 0:
            if fallback == "youtube" and _youtube_id(request):
                console.print(
                    "  [yellow]No Soulseek candidates; falling back to YouTube[/yellow]"
                )
            else:
                err(f"[yellow]{exc.message}[/yellow]")
            raise typer.Exit(0) from exc
        if fallback == "youtube" and _youtube_id(request):
            console.print(f"  [yellow]{exc.message}[/yellow]; falling back to YouTube")
            return []
        err(f"[red]{exc.message}[/red]")
        raise typer.Exit(exc.exit_code) from exc


def workflow_cmd(
    url: str = typer.Argument(..., help="YouTube URL to download and process."),
    output: Path = typer.Option(
        DEFAULT_DOWNLOAD_DIR,
        "--output",
        "-o",
        help="Directory to save downloaded files.",
    ),
    splits: Path = typer.Option(
        DEFAULT_SPLITS_DIR,
        "--splits",
        help="Directory for chapter-split tracks (album scenario only).",
    ),
    review: bool = typer.Option(
        False,
        "--review",
        "-r",
        help="Review/edit chapters in $EDITOR before splitting (album scenario only).",
    ),
    no_split: bool = typer.Option(
        False,
        "--no-split",
        help="Skip chapter splitting even when chapters are found.",
    ),
    no_organize: bool = typer.Option(
        False,
        "--no-organize",
        help="Skip beets organization.",
    ),
    import_: bool = typer.Option(
        False,
        "--import",
        "-i",
        help="Import to beets library (moves files).",
    ),
    tag_only: bool = typer.Option(
        False,
        "--tag-only",
        "-t",
        help="Only tag files with beets, do not move.",
    ),
    dry_run: bool = typer.Option(
        False,
        "--dry-run",
        "-d",
        help="Show what would happen without making changes.",
    ),
    jobs: int = typer.Option(
        0,
        "--jobs",
        "-j",
        help="Parallel ffmpeg jobs (0 = auto).",
    ),
    config: Optional[Path] = typer.Option(
        None,
        "--config",
        "-c",
        help=f"Beets config file (default: {BEETS_CONFIG}).",
    ),
    keep_source: bool = typer.Option(
        False,
        "--keep-source/--no-keep-source",
        help="Keep downloaded files after splitting (default: delete after split).",
    ),
    force: bool = typer.Option(
        False,
        "--force",
        "-f",
        help="Ignore download/split cache and reprocess from scratch.",
    ),
    metadata_source: str = typer.Option(
        "auto",
        "--metadata-source",
        help="Metadata source: youtube, musicbrainz, none, or auto.",
    ),
    audio_source: str = typer.Option(
        "youtube",
        "--audio-source",
        help="Audio source: soulseek, youtube, or auto.",
    ),
    prefer: str = typer.Option(
        "lossless",
        "--prefer",
        help="Preferred audio quality for source searches.",
    ),
    fallback: str = typer.Option(
        "youtube",
        "--fallback",
        help="Fallback source when the preferred source has no acceptable result.",
    ),
    interactive: bool = typer.Option(
        True,
        "--interactive/--no-interactive",
        help="Prompt for source candidate choices when multiple results are available.",
    ),
) -> None:
    """Full pipeline: download from YouTube → detect scenario → split/organize.

    \b
    Scenarios detected automatically per downloaded file:
      Album  — chapters found → split into tracks → beet import splits/
      Single — no chapters    → beet import file directly (no splitting)

    \b
    Playlist URLs are processed per-video: each track is fully downloaded,
    split, and organized before the next one starts, so interactive chapter
    review (--review) works naturally and a crash mid-playlist can be resumed.
    """
    console.print(f"[bold cyan]Workflow:[/bold cyan] {url}")
    console.rule()

    console.print("\n[bold]Step 1 — Download[/bold]")
    request = WorkflowRequest(raw=url, output=output, splits=splits)
    options = WorkflowOptions(
        review=review,
        no_split=no_split,
        no_organize=no_organize,
        import_=import_,
        tag_only=tag_only,
        dry_run=dry_run,
        jobs=jobs,
        config=config,
        keep_source=keep_source,
        force=force,
        metadata_source=metadata_source,
        audio_source=audio_source,
        prefer=prefer,
        fallback=fallback,
        interactive=interactive,
    )
    decisions: WorkflowDecisions = CliWorkflowDecisions(interactive=options.interactive)
    events: WorkflowEventEmitter = NullWorkflowEventEmitter()

    def download_audio(url: str, output: Path, archive_file: Path | None) -> bool:
        try:
            download_cmd(
                url=url,
                output=output,
                format="bestaudio",
                quality="0",
                no_chapters=False,
                archive_file=archive_file,
            )
        except (SystemExit, typer.Exit) as exc:
            return getattr(exc, "code", 0) == 0
        return True

    def process_audio(audio_inputs: list[Path], pre_split_dirs: list[Path]) -> None:
        _process_audio_files(
            audio_inputs=audio_inputs,
            pre_split_dirs=pre_split_dirs,
            splits=splits,
            review=review,
            no_split=no_split,
            no_organize=no_organize,
            import_=import_,
            tag_only=tag_only,
            dry_run=dry_run,
            jobs=jobs,
            config=config,
            keep_source=keep_source,
            force=force,
            decisions=decisions,
            events=events,
        )

    operations = WorkflowRunOperations(
        download_audio=download_audio,
        process_audio=process_audio,
        acquire_soulseek=lambda raw: _acquire_from_soulseek(
            raw,
            prefer=prefer,
            interactive=interactive,
            fallback=fallback,
            decisions=decisions,
            events=events,
        ),
        prepopulate_archive=_prepopulate_archive,
        get_playlist_video_ids=_get_playlist_video_ids,
    )

    try:
        run_workflow(request, options, operations=operations, events=events)
    except WorkflowServiceError as exc:
        err(f"[red]{exc.message}[/red]")
        raise typer.Exit(exc.exit_code) from exc

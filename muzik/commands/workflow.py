"""music workflow <url> — full pipeline: download → split → organize.

Handles two scenarios automatically:
  • Single track  — no chapter markers → goes straight to beets
  • Album/chapters — has chapter markers → split first, then beets

For playlist URLs the pipeline runs per-video: each track is downloaded,
classified, split (if needed), and organized before the next one starts,
so you can review/edit chapters as they arrive.
"""

import re
import subprocess
from datetime import datetime
from pathlib import Path
from typing import Optional

import typer

from muzik.commands.download import download_cmd
from muzik.commands.split import split_cmd
from muzik.commands.organize import organize_cmd
from muzik.config import AUDIO_EXTENSIONS, BEETS_CONFIG, CACHE_DIR
from muzik.core.audio import extract_metadata, get_duration
from muzik.core.chapters import find_chapters, serialize_chapters
import muzik.core.cache as cache_mod
from muzik.core.musicbrainz import MIN_ALBUM_DURATION, lookup_chapters
from muzik.ui.chapter_editor import display_chapter_table
from muzik.ui.console import console, err


def _youtube_id(url: str) -> Optional[str]:
    """Extract the 11-char YouTube video ID from a URL, or None for playlists."""
    m = re.search(r"(?:v=|youtu\.be/|/v/|/embed/)([A-Za-z0-9_-]{11})", url)
    return m.group(1) if m else None


def _playlist_id(url: str) -> Optional[str]:
    """Extract YouTube playlist ID from URL (e.g. PLxxx)."""
    m = re.search(r"[?&]list=([A-Za-z0-9_-]+)", url)
    return m.group(1) if m else None


def _video_id_from_path(path: Path) -> Optional[str]:
    """Extract 11-char YouTube video ID from filename like 'Title [ID].flac'."""
    m = re.search(r"\[([A-Za-z0-9_-]{11})\]", path.stem)
    return m.group(1) if m else None


def _load_playlist_state(playlist_id: str) -> dict:
    state = cache_mod.get_json(f"playlist_{playlist_id}") or {}
    state.setdefault("playlist_id", playlist_id)
    state.setdefault("videos", {})
    return state


def _save_playlist_state(playlist_id: str, state: dict) -> None:
    state["last_updated"] = datetime.now().isoformat(timespec="seconds")
    cache_mod.set_json(f"playlist_{playlist_id}", state)


def _prepopulate_archive(archive_file: Path) -> None:
    """Seed the yt-dlp archive with every individually-cached video ID.

    Scans all ``yt_<id>.txt`` files in the cache dir and appends any IDs not
    already present in the archive, so yt-dlp skips re-downloading songs that
    were previously processed as standalone ``muzik workflow <video-url>`` runs.
    """
    existing: set[str] = set()
    if archive_file.exists():
        for line in archive_file.read_text().splitlines():
            parts = line.strip().split()
            if len(parts) >= 2:
                existing.add(parts[1])

    new_lines: list[str] = []
    for p in CACHE_DIR.glob("yt_*.txt"):
        vid_id = p.stem[3:]  # strip "yt_" prefix
        if re.fullmatch(r"[A-Za-z0-9_-]{11}", vid_id) and vid_id not in existing:
            new_lines.append(f"youtube {vid_id}\n")

    if new_lines:
        CACHE_DIR.mkdir(parents=True, exist_ok=True)
        with archive_file.open("a") as fh:
            fh.writelines(new_lines)


def _get_playlist_video_ids(url: str) -> list[str]:
    """Return ordered video IDs in a YouTube playlist via yt-dlp."""
    result = subprocess.run(
        ["yt-dlp", "--flat-playlist", "--print", "%(id)s", url],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        return []
    return [line.strip() for line in result.stdout.splitlines() if line.strip()]


def _get_chapters_for(af: Path, no_split: bool) -> Optional[list]:
    """Return chapter list if *af* should be split as an album, else ``None``.

    Checks embedded chapters first; if none found and the file looks long enough
    to be an album, falls back to an interactive MusicBrainz lookup.
    Returns ``None`` immediately when *no_split* is set.
    """
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
    mb_chapters, mb_title = lookup_chapters(artist, album_name, year)
    if not mb_chapters:
        console.print("  [dim]MusicBrainz: no match found.[/dim]")
        return None
    console.print(
        f"  [cyan]MusicBrainz found:[/cyan] [bold]{mb_title}[/bold] — {len(mb_chapters)} tracks"
    )
    display_chapter_table(mb_chapters, title=f"MusicBrainz — {mb_title}")
    try:
        raw = input("  Use these chapters? [Y/n/e=edit]: ").strip().lower() or "y"
    except (EOFError, KeyboardInterrupt):
        raw = "n"
    if raw == "e":
        from muzik.ui.chapter_editor import edit_chapters

        mb_chapters = edit_chapters(mb_chapters) or mb_chapters
        raw = "y"
    if raw != "y":
        console.print("  [dim]Skipping MusicBrainz chapters.[/dim]")
        return None
    sidecar = af.with_suffix(".chapters.txt")
    sidecar.write_text(serialize_chapters(mb_chapters), encoding="utf-8")
    console.print(f"  [green]Saved:[/green] {sidecar.name}")
    return mb_chapters


def _find_by_id(directory: Path, yt_id: str) -> list[Path]:
    """Return audio files in *directory* whose name contains ``[yt_id]``."""
    return sorted(
        f
        for f in directory.iterdir()
        if f.is_file()
        and f.suffix.lower() in AUDIO_EXTENSIONS
        and f"[{yt_id}]" in f.name
    )


def workflow_cmd(
    url: str = typer.Argument(..., help="YouTube URL to download and process."),
    output: Path = typer.Option(
        Path("./downloads"),
        "--output",
        "-o",
        help="Directory to save downloaded files.",
    ),
    splits: Path = typer.Option(
        Path("./splits"),
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

    yt_id = _youtube_id(url)
    playlist_id = _playlist_id(url)

    # ── Playlist: per-video loop ───────────────────────────────────────────
    if playlist_id:
        archive_file = CACHE_DIR / f"ytdlp_archive_{playlist_id}.txt"
        _prepopulate_archive(archive_file)
        playlist_state = _load_playlist_state(playlist_id)
        tracked = len(playlist_state["videos"])
        console.print(
            f"  [dim]Playlist ID:[/dim] {playlist_id}"
            + (f" — {tracked} video(s) tracked" if tracked else "")
        )

        if dry_run:
            console.print(f"  [dim]Would process playlist {playlist_id} per-video[/dim]")
        else:
            console.print("  [dim]Fetching video list…[/dim]")
            video_ids = _get_playlist_video_ids(url)
            if not video_ids:
                err("[red]Could not fetch playlist video IDs — check the URL and yt-dlp.[/red]")
                raise typer.Exit(1)
            console.print(f"  [dim]{len(video_ids)} video(s) in playlist[/dim]")

            for i, vid_id in enumerate(video_ids, 1):
                console.print(
                    f"\n[bold cyan]({i}/{len(video_ids)})[/bold cyan] [dim]{vid_id}[/dim]"
                )
                console.rule(style="dim")

                entry = playlist_state["videos"].get(vid_id, {})

                # Backfill from individual cache if not yet tracked in playlist state
                if not entry:
                    ind = cache_mod.get(f"yt_{vid_id}")
                    if ind:
                        ind_path = Path(ind.strip())
                        if not ind_path.exists():
                            expected = splits / ind_path.stem
                            if expected.exists():
                                entry = {
                                    "status": "split",
                                    "audio_file": ind,
                                    "split_dir": str(expected.resolve()),
                                }
                            else:
                                entry = {"status": "organized", "audio_file": ind}

                if entry.get("status") == "organized":
                    console.print("  [green]Already organized[/green] — skipping")
                    continue

                af: Optional[Path] = None
                split_dir_for_vid: Optional[Path] = None

                # Resume from "split" state — skip straight to organize
                if entry.get("status") == "split":
                    sd = Path(entry.get("split_dir", ""))
                    if sd.exists():
                        split_dir_for_vid = sd
                        console.print(f"  [green]Already split[/green] → {sd.name}")

                # Resume from "downloaded" state or perform a fresh download
                if split_dir_for_vid is None:
                    if entry.get("status") == "downloaded":
                        cached = Path(entry["audio_file"])
                        if cached.exists():
                            af = cached
                            console.print(f"  [green]Already downloaded[/green] → {af.name}")

                    if af is None:
                        video_url = f"https://www.youtube.com/watch?v={vid_id}"
                        before = set(output.glob("*")) if output.exists() else set()
                        try:
                            download_cmd(
                                url=video_url,
                                output=output,
                                format="bestaudio",
                                quality="0",
                                no_chapters=False,
                                archive_file=archive_file,
                            )
                        except (SystemExit, typer.Exit) as exc:
                            if getattr(exc, "code", 0) != 0:
                                err(f"  [red]Download failed for {vid_id} — skipping[/red]")
                                continue

                        after = set(output.glob("*")) if output.exists() else set()
                        new_files = sorted(
                            f
                            for f in (after - before)
                            if f.is_file() and f.suffix.lower() in AUDIO_EXTENSIONS
                        )
                        if not new_files:
                            new_files = _find_by_id(output, vid_id)
                        if not new_files:
                            console.print(
                                f"  [yellow]No audio found for {vid_id} — skipping[/yellow]"
                            )
                            continue

                        af = new_files[0]
                        playlist_state["videos"][vid_id] = {
                            "status": "downloaded",
                            "audio_file": str(af.resolve()),
                        }
                        _save_playlist_state(playlist_id, playlist_state)
                        cache_mod.set(f"yt_{vid_id}", str(af.resolve()))

                    # Classify and split
                    chapters = _get_chapters_for(af, no_split)
                    if chapters:
                        console.print(f"\n[bold]Split[/bold] — {len(chapters)} chapters")
                        out_dir = splits / af.stem
                        try:
                            split_cmd(
                                path=af,
                                review=review,
                                jobs=jobs,
                                output=out_dir,
                                keep_source=keep_source,
                            )
                        except (SystemExit, typer.Exit) as exc:
                            if getattr(exc, "code", 0) != 0:
                                err(f"  [red]Split failed for {af.name}[/red]")
                                continue
                        if out_dir.exists():
                            split_dir_for_vid = out_dir
                            playlist_state["videos"][vid_id]["status"] = "split"
                            playlist_state["videos"][vid_id]["split_dir"] = str(
                                out_dir.resolve()
                            )
                            _save_playlist_state(playlist_id, playlist_state)

                    # Single track (no chapters, or split failed / no out_dir)
                    if split_dir_for_vid is None:
                        if not no_organize:
                            console.print(f"\n[bold]Organize[/bold] — {af.name}")
                            try:
                                organize_cmd(
                                    directory=af,
                                    import_=import_,
                                    tag_only=tag_only,
                                    dry_run=dry_run,
                                    config=config,
                                )
                            except (SystemExit, typer.Exit) as exc:
                                if getattr(exc, "code", 0) != 0:
                                    err(f"  [red]beet failed for {af.name}[/red]")
                                    continue
                            playlist_state["videos"][vid_id]["status"] = "organized"
                            _save_playlist_state(playlist_id, playlist_state)
                        continue  # move to next video regardless

                # Organize the split directory
                if split_dir_for_vid is not None and not no_organize:
                    console.print(f"\n[bold]Organize[/bold] — {split_dir_for_vid.name}/")
                    try:
                        organize_cmd(
                            directory=split_dir_for_vid,
                            import_=import_,
                            tag_only=tag_only,
                            dry_run=dry_run,
                            config=config,
                        )
                    except (SystemExit, typer.Exit) as exc:
                        if getattr(exc, "code", 0) != 0:
                            err(f"  [red]beet failed for {split_dir_for_vid}[/red]")
                            continue
                    playlist_state["videos"][vid_id]["status"] = "organized"
                    _save_playlist_state(playlist_id, playlist_state)

        console.rule()
        console.print("[bold green]Workflow complete.[/bold green]")
        return

    # ── Single video: existing batch flow ─────────────────────────────────
    audio_files: list[Path] = []
    pre_split_dirs: list[Path] = []

    if dry_run:
        console.print(f"  [dim]Would download {url} → {output}[/dim]")
    else:
        cache_key = f"yt_{yt_id}" if yt_id else None

        # Check persistent cache first — records downloads even after source is deleted
        cached_entry = cache_mod.get(cache_key) if cache_key else None
        if cached_entry:
            cached_path = Path(cached_entry.strip())
            if cached_path.exists():
                audio_files = [cached_path]
                console.print(
                    f"  [green]Cached download[/green] — skipping yt-dlp\n"
                    f"  [dim]{cached_path.name}[/dim]"
                )
            else:
                # Source was deleted after splitting — skip re-download and look for
                # the existing split directory so we can still reach Step 3.
                console.print(
                    f"  [green]Already downloaded[/green] (source deleted after split,"
                    f" skipping yt-dlp)\n  [dim]{cached_path.name}[/dim]"
                )
                expected_split = splits / cached_path.stem
                if expected_split.exists():
                    pre_split_dirs.append(expected_split)
                    console.print(
                        f"  [dim]Found existing split dir: {expected_split.name}[/dim]"
                    )
        else:
            # Not in cache — check output dir by ID (fast, avoids yt-dlp)
            if yt_id and output.exists():
                audio_files = _find_by_id(output, yt_id)
                if audio_files:
                    console.print(
                        "  [green]Already downloaded[/green] — skipping yt-dlp\n"
                        + "\n".join(f"  [dim]{f.name}[/dim]" for f in audio_files)
                    )
                    if cache_key:
                        cache_mod.set(cache_key, str(audio_files[0]))

            if not audio_files:
                before = set(output.glob("*")) if output.exists() else set()
                try:
                    download_cmd(
                        url=url,
                        output=output,
                        format="bestaudio",
                        quality="0",
                        no_chapters=False,
                        archive_file=None,
                    )
                except (SystemExit, typer.Exit) as exc:
                    if getattr(exc, "code", 0) != 0:
                        err("[red]Download failed. Aborting workflow.[/red]")
                        raise typer.Exit(1)

                after = set(output.glob("*")) if output.exists() else set()
                audio_files = sorted(
                    f
                    for f in (after - before)
                    if f.is_file() and f.suffix.lower() in AUDIO_EXTENSIONS
                )
                # Last resort: ID-based lookup (handles yt-dlp format quirks)
                if not audio_files and yt_id and output.exists():
                    audio_files = _find_by_id(output, yt_id)

                # Persist to cache so future runs skip yt-dlp
                if audio_files and cache_key:
                    cache_mod.set(cache_key, str(audio_files[0]))

    if not audio_files and not pre_split_dirs and not dry_run:
        err("[yellow]No audio files found in output directory.[/yellow]")
        raise typer.Exit(0)

    # ── Classify each file ────────────────────────────────────────────────
    albums: list[tuple[Path, list]] = []  # (file, chapters)
    singles: list[Path] = []

    for af in audio_files:
        chapters = _get_chapters_for(af, no_split)
        if chapters:
            albums.append((af, chapters))
        else:
            singles.append(af)

    if albums:
        console.print(
            f"\n  [cyan]Album(s) detected:[/cyan] "
            f"{len(albums)} file(s) with chapter markers"
        )
    if singles:
        console.print(
            f"\n  [yellow]Single track(s) detected:[/yellow] "
            f"{len(singles)} file(s) without chapters"
        )

    # ── Step 2a: Album path — split by chapters ───────────────────────────
    # Pre-populate with dirs discovered in Step 1 (source deleted, split already done)
    split_dirs: list[Path] = list(pre_split_dirs)

    if albums:
        console.print("\n[bold]Step 2a — Split (album)[/bold]")

        for af, chapters in albums:
            console.print(f"  [cyan]{af.name}[/cyan] — {len(chapters)} chapters")
            out_dir = splits / af.stem
            if dry_run:
                console.print(f"  [dim]Would split → {out_dir}[/dim]")
            else:
                try:
                    split_cmd(
                        path=af,
                        review=review,
                        jobs=jobs,
                        output=out_dir,
                        keep_source=keep_source,
                    )
                except (SystemExit, typer.Exit) as exc:
                    if getattr(exc, "code", 0) != 0:
                        err(f"  [red]Split failed for {af.name}[/red]")
                        continue
                # Add dir whether freshly split or served from cache (SystemExit(0))
                if out_dir.exists():
                    split_dirs.append(out_dir)

    # ── Step 2b: Single track path — nothing to split ─────────────────────
    if singles:
        console.print("\n[bold]Step 2b — Single track(s)[/bold]")
        for af in singles:
            console.print(
                f"  [yellow]{af.name}[/yellow] — no chapters, will organize directly"
            )

    # ── Step 3: Organize ──────────────────────────────────────────────────
    if no_organize:
        console.rule()
        console.print("[bold green]Workflow complete (organize skipped).[/bold green]")
        return

    console.print("\n[bold]Step 3 — Organize[/bold]")

    # Organize split tracks (album scenario)
    for sd in split_dirs:
        console.print(f"  beet import [dim]{sd}[/dim]")
        if not dry_run:
            try:
                organize_cmd(
                    directory=sd,
                    import_=import_,
                    tag_only=tag_only,
                    dry_run=dry_run,
                    config=config,
                )
            except (SystemExit, typer.Exit) as exc:
                if getattr(exc, "code", 0) != 0:
                    err(f"  [red]beet failed for {sd}[/red]")

    # Organize single tracks directly — pass the file path; beet import accepts files too
    for af in singles:
        console.print(f"  beet import [dim]{af}[/dim]")
        if not dry_run:
            try:
                organize_cmd(
                    directory=af,
                    import_=import_,
                    tag_only=tag_only,
                    dry_run=dry_run,
                    config=config,
                )
            except (SystemExit, typer.Exit) as exc:
                if getattr(exc, "code", 0) != 0:
                    err(f"  [red]beet failed for {af.name}[/red]")

    console.rule()
    console.print("[bold green]Workflow complete.[/bold green]")

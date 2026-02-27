"""music archive <dir> — process existing downloaded files (split + organize)."""

from pathlib import Path
from typing import Optional

import typer

from muzik.commands.split import split_cmd
from muzik.commands.organize import organize_cmd
from muzik.config import AUDIO_EXTENSIONS, BEETS_CONFIG
from muzik.core.chapters import find_chapters
from muzik.ui.console import console, err


def archive_cmd(
    directory: Path = typer.Argument(
        ..., help="Directory of already-downloaded audio files."
    ),
    output: Path = typer.Option(
        Path("./splits"),
        "--output",
        "-o",
        help="Output directory for split tracks.",
    ),
    import_: bool = typer.Option(
        False,
        "--import",
        "-i",
        help="Import split tracks into beets library.",
    ),
    tag_only: bool = typer.Option(
        False,
        "--tag-only",
        "-t",
        help="Only tag files, do not move them.",
    ),
    dry_run: bool = typer.Option(
        False,
        "--dry-run",
        "-d",
        help="Show what would be done without making changes.",
    ),
    skip_split: bool = typer.Option(
        False,
        "--skip-split",
        help="Skip chapter splitting; go straight to beets.",
    ),
    skip_organize: bool = typer.Option(
        False,
        "--skip-organize",
        help="Skip beets organization after splitting.",
    ),
    jobs: int = typer.Option(
        0,
        "--jobs",
        "-j",
        help="Parallel ffmpeg jobs per file (0 = auto).",
    ),
    keep_source: bool = typer.Option(
        False,
        "--keep-source",
        help="Keep original audio files after splitting.",
    ),
    config: Optional[Path] = typer.Option(
        None,
        "--config",
        "-c",
        help=f"Beets config file (default: {BEETS_CONFIG}).",
    ),
) -> None:
    """Process existing downloaded audio files: split by chapters, then organize."""
    if not directory.exists():
        err(f"[red]Directory not found: {directory}[/red]")
        raise typer.Exit(1)

    audio_files = [
        f
        for f in sorted(directory.iterdir())
        if f.is_file() and f.suffix.lower() in AUDIO_EXTENSIONS
    ]

    if not audio_files:
        console.print(f"[yellow]No audio files found in {directory}[/yellow]")
        raise typer.Exit(0)

    console.print(
        f"[bold]Archive:[/bold] {directory}  "
        f"([dim]{len(audio_files)} audio file(s)[/dim])"
    )

    # --- Split phase ---
    if not skip_split:
        processed = 0
        skipped = 0

        for af in audio_files:
            chapters = find_chapters(af)

            if not chapters:
                console.print(f"[dim]  {af.name} — no chapters found, skipping[/dim]")
                skipped += 1
                continue

            console.print(
                f"\n[cyan]  Splitting:[/cyan] {af.name} "
                f"([dim]{len(chapters)} tracks[/dim])"
            )

            if dry_run:
                console.print(f"  [dim]Would split → {output / af.stem}[/dim]")
            else:
                # Delegate to split_cmd (uses a standalone Typer context)
                try:
                    split_cmd(
                        path=af,
                        review=False,
                        jobs=jobs,
                        output=output / af.stem,
                        keep_source=keep_source,
                    )
                    processed += 1
                except SystemExit as exc:
                    if exc.code != 0:
                        err(f"  [red]Split failed for {af.name}[/red]")

        console.print(
            f"\n[bold]Split summary:[/bold] {processed} processed, {skipped} skipped"
        )

    # --- Organize phase ---
    if not skip_organize:
        if not output.exists() or not any(output.iterdir()):
            console.print(
                f"[yellow]No split output found in {output}, skipping beets.[/yellow]"
            )
            raise typer.Exit(0)

        console.print(f"\n[bold]Organizing:[/bold] {output}")

        if dry_run:
            console.print("[dim]  Would run beet (dry-run).[/dim]")
        else:
            try:
                organize_cmd(
                    directory=output,
                    import_=import_,
                    tag_only=tag_only,
                    dry_run=dry_run,
                    config=config,
                )
            except SystemExit as exc:
                if exc.code != 0:
                    err("[red]beet organization failed.[/red]")
                    raise typer.Exit(1)

    console.print("\n[green]Archive processing complete.[/green]")

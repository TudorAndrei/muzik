"""music validate <path> — audio/chapters/metadata file validator."""

from pathlib import Path

import typer
from rich.table import Table

from muzik.config import AUDIO_EXTENSIONS
from muzik.core.audio import probe
from muzik.core.chapters import parse_chapters_txt
from muzik.ui.console import console, err


def validate_cmd(
    path: Path = typer.Argument(..., help="File or directory to validate."),
    recursive: bool = typer.Option(
        False,
        "--recursive",
        "-r",
        help="Recursively scan directories.",
    ),
    verbose: bool = typer.Option(
        False,
        "--verbose",
        "-v",
        help="Show per-file details (duration, codec, chapter count…).",
    ),
) -> None:
    """Validate audio files, chapter sidecars, and metadata (info.json)."""
    if not path.exists():
        err(f"[red]Not found: {path}[/red]")
        raise typer.Exit(1)

    files: list[Path] = []
    if path.is_file():
        files = [path]
    elif recursive:
        files = sorted(path.rglob("*"))
    else:
        files = sorted(p for p in path.iterdir() if p.is_file())

    # Only care about known file types
    relevant = [
        f
        for f in files
        if f.suffix.lower() in AUDIO_EXTENSIONS
        or f.name.endswith(".chapters.txt")
        or f.name.endswith(".info.json")
    ]

    if not relevant:
        console.print("[yellow]No relevant files found.[/yellow]")
        raise typer.Exit(0)

    table = Table(
        title=f"Validation — {path}",
        show_header=True,
        header_style="bold cyan",
        border_style="dim",
    )
    table.add_column("File", overflow="fold")
    table.add_column("Type", width=8)
    table.add_column("Status", width=8)
    if verbose:
        table.add_column("Details", overflow="fold")

    valid_count = 0
    invalid_count = 0

    for f in relevant:
        status = "[green]OK[/green]"
        details = ""
        file_type = ""

        try:
            if f.suffix.lower() in AUDIO_EXTENSIONS:
                file_type = "audio"
                data = probe(f)
                if verbose:
                    fmt = data.get("format", {})
                    streams = data.get("streams", [{}])
                    codec = streams[0].get("codec_name", "?") if streams else "?"
                    dur = float(fmt.get("duration", 0))
                    mm, ss = divmod(int(dur), 60)
                    hh, mm = divmod(mm, 60)
                    details = f"codec={codec} dur={hh:02d}:{mm:02d}:{ss:02d}"

            elif f.name.endswith(".chapters.txt"):
                file_type = "chapters"
                chapters = parse_chapters_txt(f)
                if not chapters:
                    raise ValueError("No valid chapter lines found")
                if verbose:
                    details = f"{len(chapters)} chapters"

            elif f.name.endswith(".info.json"):
                file_type = "info.json"
                import json

                data = json.loads(f.read_text(encoding="utf-8", errors="replace"))
                if not isinstance(data, dict):
                    raise ValueError("Root is not a JSON object")
                if verbose:
                    title = data.get("title", "?")
                    ch_count = len(data.get("chapters") or [])
                    details = f"title={title!r} chapters={ch_count}"

            valid_count += 1

        except Exception as exc:
            status = "[red]FAIL[/red]"
            details = str(exc)[:80]
            invalid_count += 1

        rel = f.relative_to(path) if path.is_dir() else f.name
        row = [str(rel), file_type, status]
        if verbose:
            row.append(details)
        table.add_row(*row)

    console.print(table)

    summary_color = "green" if invalid_count == 0 else "red"
    console.print(
        f"[{summary_color}]"
        f"{valid_count} valid, {invalid_count} invalid "
        f"({len(relevant)} files checked)"
        f"[/{summary_color}]"
    )

    if invalid_count:
        raise typer.Exit(1)

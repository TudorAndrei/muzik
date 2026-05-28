"""music validate <path> — audio/chapters/metadata file validator."""

from pathlib import Path

import typer
from rich.table import Table

from muzik.config import AUDIO_EXTENSIONS
from muzik.core.audio import probe
from muzik.core.chapters import parse_chapters_txt
from muzik.core.metadata import read_muzik_metadata
from muzik.core.quality import is_lossless, quality_from_name
from muzik.ui.console import console, err


def _metadata_quality_details(data: dict) -> tuple[str, list[str]]:
    details: list[str] = []
    warnings: list[str] = []

    source = data.get("source")
    if source:
        details.append(f"source={source}")
    source_id = data.get("source_id")
    if source_id:
        details.append(f"source_id={source_id}")
    prefer = data.get("prefer") or data.get("preferred_format")
    if prefer:
        details.append(f"prefer={prefer}")

    candidate = data.get("candidate") or {}
    if isinstance(candidate, dict):
        quality = candidate.get("quality") or {}
        if isinstance(quality, dict):
            fmt = quality.get("format")
            lossless = bool(quality.get("lossless"))
            if fmt:
                details.append(f"format={fmt}")
                details.append(f"lossless={'yes' if lossless else 'no'}")

            requested = str(data.get("requested") or "").lower()
            prefer = str(prefer or "")
            wants_lossless = (
                "lossless" in prefer.lower()
                or "flac" in prefer.lower()
                or "flac" in requested
            )
            if wants_lossless and fmt and not lossless:
                warnings.append("requested lossless but metadata says lossy")

    return " ".join(details), warnings


def _audio_quality_details(path: Path, metadata: dict | None) -> tuple[str, list[str]]:
    quality = quality_from_name(path.name)
    details = [
        f"detected={quality.format or '?'}",
        f"lossless={'yes' if is_lossless(quality.format) else 'no'}",
    ]
    warnings: list[str] = []

    if metadata:
        meta_details, meta_warnings = _metadata_quality_details(metadata)
        if meta_details:
            details.append(meta_details)
        warnings.extend(meta_warnings)
    else:
        warnings.append("metadata sidecar missing")

    return " ".join(details), warnings


def _album_completeness_warnings(sidecar: Path, metadata: dict) -> list[str]:
    candidate = metadata.get("candidate") or {}
    if not isinstance(candidate, dict):
        return []
    expected_files = candidate.get("files") or []
    if not isinstance(expected_files, list) or not expected_files:
        return []

    root = sidecar.parent
    if sidecar.name != ".muzik.json":
        root = sidecar.parent
    actual_audio = [
        file
        for file in root.rglob("*")
        if file.is_file() and file.suffix.lower() in AUDIO_EXTENSIONS
    ]
    if len(actual_audio) < len(expected_files):
        return [
            f"album appears incomplete ({len(actual_audio)}/{len(expected_files)} audio files)"
        ]
    return []


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
    """Validate audio files, chapter sidecars, and metadata sidecars."""
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
        or f.name.endswith(".muzik.json")
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
    warn_count = 0

    for f in relevant:
        status = "[green]OK[/green]"
        details = ""
        file_type = ""
        warnings: list[str] = []

        try:
            if f.suffix.lower() in AUDIO_EXTENSIONS:
                file_type = "audio"
                data = probe(f)
                metadata = read_muzik_metadata(f)
                if verbose:
                    fmt = data.get("format", {})
                    streams = data.get("streams", [{}])
                    codec = streams[0].get("codec_name", "?") if streams else "?"
                    dur = float(fmt.get("duration", 0))
                    mm, ss = divmod(int(dur), 60)
                    hh, mm = divmod(mm, 60)
                    quality_details, warnings = _audio_quality_details(f, metadata)
                    details = (
                        f"codec={codec} dur={hh:02d}:{mm:02d}:{ss:02d} "
                        f"{quality_details}"
                    )
                elif metadata is None:
                    warnings.append("metadata sidecar missing")

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

            elif f.name.endswith(".muzik.json"):
                file_type = "muzik"
                data = read_muzik_metadata(f)
                if not isinstance(data, dict):
                    raise ValueError("Root is not a JSON object")
                if not data.get("source"):
                    warnings.append("missing source")
                if not data.get("source_id"):
                    warnings.append("missing source_id")
                if verbose:
                    quality_details, quality_warnings = _metadata_quality_details(data)
                    warnings.extend(quality_warnings)
                    warnings.extend(_album_completeness_warnings(f, data))
                    details = quality_details or f"source={data.get('source', '?')}"
                else:
                    warnings.extend(_album_completeness_warnings(f, data))

            if warnings:
                status = "[yellow]WARN[/yellow]"
                warn_count += 1
                if verbose:
                    suffix = "; ".join(warnings)
                    details = f"{details}; {suffix}" if details else suffix

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
        f"{valid_count} valid, {warn_count} warnings, {invalid_count} invalid "
        f"({len(relevant)} files checked)"
        f"[/{summary_color}]"
    )

    if invalid_count:
        raise typer.Exit(1)

"""muzik downloaded — list audio already in the output folder."""

from __future__ import annotations

import datetime
from pathlib import Path

import typer
from rich.table import Table

from muzik.config import DEFAULT_DOWNLOAD_DIR
from muzik.core.library import DownloadedItem, human_size, scan_downloads
from muzik.ui.console import console


def downloaded_cmd(
    output: Path = typer.Option(
        DEFAULT_DOWNLOAD_DIR,
        "--output",
        "-o",
        help="Output folder to inventory.",
    ),
) -> None:
    """List downloaded audio so the same URL is not fetched twice."""
    items = scan_downloads(output)
    if not items:
        console.print(f"[dim]No downloads found.[/dim]  ({output})")
        return

    table = Table(
        title=f"Downloaded — {output}",
        show_header=True,
        header_style="bold cyan",
        border_style="dim",
    )
    table.add_column("Title", style="bold", no_wrap=False)
    table.add_column("YouTube ID", style="dim")
    table.add_column("Format")
    table.add_column("Size", justify="right")
    table.add_column("Modified")

    total_bytes = 0
    with_id = 0
    for item in items:
        total_bytes += item.size
        with_id += 1 if item.youtube_id else 0
        table.add_row(*_row(item))

    console.print(table)
    console.print(
        f"[dim]Total: {len(items)} file(s), {human_size(total_bytes)}; "
        f"{with_id} with a YouTube id.[/dim]"
    )


def _row(item: DownloadedItem) -> tuple[str, str, str, str, str]:
    modified = datetime.datetime.fromtimestamp(item.mtime).strftime("%Y-%m-%d %H:%M")
    return (
        item.title,
        item.youtube_id or "—",
        item.ext,
        human_size(item.size),
        modified,
    )

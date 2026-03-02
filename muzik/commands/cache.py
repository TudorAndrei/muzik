"""music cache [list|clear|size|clean] — cache manager."""

from typing import Optional

import typer
from rich.table import Table

from muzik.config import CACHE_DIR, DEFAULT_DOWNLOAD_DIR, DEFAULT_SPLITS_DIR
from muzik.core import cache as cache_mod
from muzik.ui.console import console, err

app = typer.Typer(help="Manage the ~/.cache/muzik cache.")


def _human_size(n: int) -> str:
    for unit in ("B", "KB", "MB", "GB"):
        if n < 1024:
            return f"{n:.1f} {unit}"
        n /= 1024  # type: ignore[assignment]
    return f"{n:.1f} TB"


@app.command("list")
def cache_list() -> None:
    """List all cached items with size and modification date."""
    files = cache_mod.list_all()

    if not files:
        console.print(f"[dim]Cache is empty.[/dim]  ({CACHE_DIR})")
        return

    table = Table(
        title=f"Cache — {CACHE_DIR}",
        show_header=True,
        header_style="bold cyan",
        border_style="dim",
    )
    table.add_column("Key", style="bold")
    table.add_column("Size", justify="right")
    table.add_column("Modified")
    table.add_column("Ext", style="dim")

    import datetime

    for p in files:
        stem = p.stem
        ext = p.suffix.lstrip(".")
        size = _human_size(p.stat().st_size)
        mtime = datetime.datetime.fromtimestamp(p.stat().st_mtime)
        modified = mtime.strftime("%Y-%m-%d %H:%M")
        table.add_row(stem, size, modified, ext)

    console.print(table)
    console.print(
        f"[dim]Total: {len(files)} file(s), {_human_size(cache_mod.total_size())}[/dim]"
    )


@app.command("clear")
def cache_clear(
    key: Optional[str] = typer.Argument(
        None, help="Cache key to clear (omit to clear ALL entries)."
    ),
) -> None:
    """Clear a specific cache entry or all entries."""
    if key is None:
        files = cache_mod.list_all()
        if not files:
            console.print("[dim]Cache is already empty.[/dim]")
            return
        typer.confirm(f"Delete all {len(files)} cache entries?", abort=True)
        for p in files:
            p.unlink(missing_ok=True)
        console.print(f"[green]Cleared {len(files)} cache entries.[/green]")
    else:
        # Try all extensions
        removed = False
        for ext in ("txt", "json"):
            if cache_mod.delete(key, ext):
                removed = True
        if removed:
            console.print(f"[green]Cleared cache entry: {key}[/green]")
        else:
            err(f"[yellow]Cache entry not found: {key}[/yellow]")


@app.command("size")
def cache_size() -> None:
    """Show total cache directory size."""
    files = cache_mod.list_all()
    total = cache_mod.total_size()
    console.print(
        f"[bold]Cache:[/bold] {CACHE_DIR}\n  {len(files)} file(s), {_human_size(total)}"
    )


@app.command("purge")
def cache_purge() -> None:
    """Delete all cache entries and all downloaded files."""
    import shutil

    cache_files = cache_mod.list_all()
    dl_files = (
        sorted(DEFAULT_DOWNLOAD_DIR.iterdir()) if DEFAULT_DOWNLOAD_DIR.exists() else []
    )
    split_files = (
        sorted(DEFAULT_SPLITS_DIR.iterdir()) if DEFAULT_SPLITS_DIR.exists() else []
    )

    if not cache_files and not dl_files and not split_files:
        console.print("[dim]Nothing to purge.[/dim]")
        return

    lines = []
    if cache_files:
        lines.append(f"{len(cache_files)} cache entry/entries ({CACHE_DIR})")
    if dl_files:
        lines.append(f"{len(dl_files)} downloaded file(s) ({DEFAULT_DOWNLOAD_DIR})")
    if split_files:
        lines.append(f"{len(split_files)} split dir(s) ({DEFAULT_SPLITS_DIR})")

    console.print("[bold red]This will permanently delete:[/bold red]")
    for line in lines:
        console.print(f"  • {line}")

    typer.confirm("Continue?", abort=True)

    for p in cache_files:
        p.unlink(missing_ok=True)

    if DEFAULT_DOWNLOAD_DIR.exists():
        shutil.rmtree(DEFAULT_DOWNLOAD_DIR)
        DEFAULT_DOWNLOAD_DIR.mkdir(parents=True, exist_ok=True)

    if DEFAULT_SPLITS_DIR.exists():
        shutil.rmtree(DEFAULT_SPLITS_DIR)
        DEFAULT_SPLITS_DIR.mkdir(parents=True, exist_ok=True)

    console.print("[green]Purge complete.[/green]")


@app.command("clean")
def cache_clean(
    max_age: int = typer.Option(
        30, "--max-age", help="Remove entries older than this many days."
    ),
) -> None:
    """Remove empty and old cache entries."""
    removed = cache_mod.clean(max_age_days=max_age)
    if removed:
        console.print(f"[green]Removed {removed} stale cache entries.[/green]")
    else:
        console.print("[dim]No stale entries found.[/dim]")

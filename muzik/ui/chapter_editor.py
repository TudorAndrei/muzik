"""Interactive chapter review and editor.

``edit_chapters()`` flow:
1. Display a Rich Table (columns: #, Start, Duration, Title).
2. Prompt: [c]ontinue / [e]dit / [a]bort.
3. If edit: write to NamedTemporaryFile, open $EDITOR, re-parse, repeat.
"""

import os
import shutil
import subprocess
import tempfile
from pathlib import Path

from rich.table import Table

from muzik.core.chapters import Chapter, parse_chapters_txt, serialize_chapters
from muzik.ui.console import console


def display_chapter_table(chapters: list[Chapter], title: str = "Chapters") -> None:
    """Print a Rich table of *chapters* to the console."""
    table = Table(
        title=title, show_header=True, header_style="bold cyan", border_style="dim"
    )
    table.add_column("#", style="dim", width=4, justify="right")
    table.add_column("Start", width=10)
    table.add_column("Duration", width=10)
    table.add_column("Title")

    for ch in chapters:
        table.add_row(
            str(ch.index),
            ch.start_ts,
            ch.duration_str,
            ch.title,
        )

    console.print(table)


def _find_editor() -> str:
    """Return a usable editor command."""
    for env_var in ("EDITOR", "VISUAL"):
        editor = os.environ.get(env_var, "").strip()
        if editor and shutil.which(editor.split()[0]):
            return editor
    for fallback in ("nano", "vi", "vim"):
        if shutil.which(fallback):
            return fallback
    return "vi"


def edit_chapters(chapters: list[Chapter]) -> list[Chapter] | None:
    """Interactive chapter editor loop.

    Returns the (possibly updated) chapter list when the user continues,
    or ``None`` if the user aborts.
    """
    while True:
        display_chapter_table(chapters)

        console.print(
            "\n  [bold][c][/bold]ontinue  [bold][e][/bold]dit  [bold][a][/bold]bort",
        )
        try:
            raw = input("  Choice [c]: ").strip().lower() or "c"
        except (EOFError, KeyboardInterrupt):
            console.print("\n[yellow]Aborted.[/yellow]")
            return None

        if raw == "c":
            return chapters

        if raw == "a":
            console.print("[yellow]Aborted.[/yellow]")
            return None

        if raw == "e":
            # Write current chapters to a temp file and open editor
            tmp = tempfile.NamedTemporaryFile(
                mode="w",
                suffix=".chapters.txt",
                prefix="music-chapters-",
                delete=False,
                encoding="utf-8",
            )
            try:
                tmp.write(serialize_chapters(chapters))
                tmp.flush()
                tmp_path = Path(tmp.name)
            finally:
                tmp.close()

            editor = _find_editor()
            console.print(f"[dim]Opening {tmp_path} in {editor!r}…[/dim]")
            # Run editor blocking — inherit terminal
            subprocess.run([*editor.split(), str(tmp_path)])

            try:
                updated = parse_chapters_txt(tmp_path)
                if not updated:
                    console.print(
                        "[red]No valid chapters found after editing — keeping original.[/red]"
                    )
                else:
                    # Re-derive end times from successive starts
                    for i in range(len(updated) - 1):
                        updated[i] = Chapter(
                            index=i + 1,
                            start=updated[i].start,
                            end=updated[i + 1].start,
                            title=updated[i].title,
                        )
                    updated[-1] = Chapter(
                        index=len(updated),
                        start=updated[-1].start,
                        end=None,
                        title=updated[-1].title,
                    )
                    chapters = updated
                    console.print(
                        f"[green]Chapters updated ({len(chapters)} tracks).[/green]"
                    )
            except Exception as exc:
                console.print(f"[red]Error parsing edited chapters: {exc}[/red]")
            finally:
                tmp_path.unlink(missing_ok=True)
        else:
            console.print("[dim]Please enter c, e, or a.[/dim]")

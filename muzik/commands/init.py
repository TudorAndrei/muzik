"""Create app directories and the music library config."""

import re


from muzik.config import (
    LIBRARY_CONFIG,
    CACHE_DIR,
    DEFAULT_DOWNLOAD_DIR,
    DEFAULT_SOULSEEK_DIR,
    DEFAULT_SPLITS_DIR,
    SEAKARR_DOWNLOAD_DIR,
    SEAKARR_PASSWORD,
    SEAKARR_USERNAME,
)
from muzik.ui.console import console


# Import settings shared with existing library configs.
_IMPORT_BLOCK = (
    "import:\n"
    "  move: yes\n"
    "  duplicate_action: skip\n"
    "  none_rec_action: asis\n"
    "match:\n"
    "  strong_rec_thresh: 0.10\n"
    "  medium_rec_thresh: 0.20\n"
)


def _ensure_dirs() -> None:
    dirs = {
        "Downloads": DEFAULT_DOWNLOAD_DIR,
        "Soulseek ": DEFAULT_SOULSEEK_DIR,
        "Splits   ": DEFAULT_SPLITS_DIR,
        "Cache    ": CACHE_DIR,
        "Library cfg": LIBRARY_CONFIG.parent,
    }
    for label, d in dirs.items():
        existed = d.exists()
        d.mkdir(parents=True, exist_ok=True)
        status = "[dim]already exists[/dim]" if existed else "[green]created[/green]"
        console.print(f"  {label}  {d}  {status}")


def _configure_library() -> None:
    """Ensure the library config contains import defaults.

    • If the config file doesn't exist yet, a minimal one is created.
    • If it already has the required import settings, nothing is changed.
    • If it has an ``import:`` section, any missing required keys are inserted
      right after ``import:``.
    • Otherwise the full ``import:`` block is appended.
    """
    cfg = LIBRARY_CONFIG

    if not cfg.exists():
        cfg.write_text(
            "# Music library config created by muzik init\n\n" + _IMPORT_BLOCK
        )
        console.print(f"  Library cfg  {cfg}  [green]created[/green]")
        return

    text = cfg.read_text()
    changed: list[str] = []

    has_import_defaults = (
        "move:" in text and "duplicate_action" in text and "none_rec_action" in text
    )
    if not has_import_defaults:
        text = _add_import_defaults(text)
        changed.append("import defaults")

    if not changed:
        console.print(f"  Library cfg  {cfg}  [dim]already set — skipped[/dim]")
        return
    cfg.write_text(text)
    console.print(f"  Library cfg  {cfg}  [green]added {', '.join(changed)}[/green]")


def _add_import_defaults(text: str) -> str:
    if re.search(r"^import\s*:", text, re.MULTILINE):
        missing = []
        if not re.search(r"^\s+move\s*:", text, re.MULTILINE):
            missing.append("  move: yes")
        if not re.search(r"^\s+duplicate_action\s*:", text, re.MULTILINE):
            missing.append("  duplicate_action: skip")
        if not re.search(r"^\s+none_rec_action\s*:", text, re.MULTILINE):
            missing.append("  none_rec_action: asis")
        if missing:
            text = re.sub(
                r"(^import\s*:[ \t]*$)",
                "\\1\n" + "\n".join(missing),
                text,
                count=1,
                flags=re.MULTILINE,
            )
        return text
    return text.rstrip("\n") + "\n\n" + _IMPORT_BLOCK


def init_cmd() -> None:
    """Create app directories and configure the music library.

    \b
    Creates:
      platform user data dir/downloads   — default download directory
      platform user data dir/soulseek    — default Soulseek download directory
      platform user data dir/splits      — default splits directory
      platform user cache dir            — cache directory
      library config dir                 — music library config directory

    \b
    Library config changes:
      Sets import.move = yes so imports move files into the music library.
      Sets import.duplicate_action = skip so that albums already present in
      the library are silently skipped on every workflow re-run.
      Existing settings are preserved; the file is only written if the
      setting is missing.
    """
    console.print("[bold]Directories[/bold]")
    _ensure_dirs()

    console.print("\n[bold]Library configuration[/bold]")
    _configure_library()

    console.print("\n[bold]Soulseek configuration[/bold]")
    console.print(f"  MUZIK_SOULSEEK_DOWNLOAD_DIR  [dim]{SEAKARR_DOWNLOAD_DIR}[/dim]")
    if SEAKARR_USERNAME and SEAKARR_PASSWORD:
        console.print("  MUZIK_SOULSEEK_USERNAME      [green]set[/green]")
        console.print("  MUZIK_SOULSEEK_PASSWORD      [green]set[/green]")
    else:
        console.print("  MUZIK_SOULSEEK_USERNAME      [yellow]not set[/yellow]")
        console.print("  MUZIK_SOULSEEK_PASSWORD      [yellow]not set[/yellow]")
        console.print(
            "  [dim]Set MUZIK_SOULSEEK_USERNAME and MUZIK_SOULSEEK_PASSWORD "
            "to use Soulseek.[/dim]"
        )

    console.rule()
    console.print("[bold green]muzik init complete.[/bold green]")
    console.print(
        "\n[dim]Run [bold]muzik workflow <url>[/bold] to start downloading.[/dim]"
    )

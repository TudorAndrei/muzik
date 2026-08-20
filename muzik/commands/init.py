"""muzik init — create app directories and configure beets for use with muzik."""

import re


from muzik.config import (
    BEETS_CONFIG,
    CACHE_DIR,
    DEFAULT_DOWNLOAD_DIR,
    DEFAULT_SOULSEEK_DIR,
    DEFAULT_SPLITS_DIR,
    SLSKD_API_KEY,
    SLSKD_DOWNLOAD_DIR,
    SLSKD_URL,
)
from muzik.ui.console import console


# The beets import setting muzik requires.
_IMPORT_BLOCK = (
    "import:\n"
    "  move: yes\n"
    "  duplicate_action: skip\n"
    "  none_rec_action: asis\n"
    "match:\n"
    "  strong_rec_thresh: 0.10\n"
    "  medium_rec_thresh: 0.20\n"
)

# Album-art plugins. musicbrainz stays enabled (it is the default source);
# fetchart picks up the cover.jpg the workflow drops beside split albums, and
# embedart writes it into each track so it travels with the files.
_PLUGINS_BLOCK = (
    "plugins: musicbrainz fetchart embedart\n"
    "fetchart:\n"
    "  sources: filesystem\n"
    "embedart:\n"
    "  auto: yes\n"
)


def _ensure_dirs() -> None:
    dirs = {
        "Downloads": DEFAULT_DOWNLOAD_DIR,
        "Soulseek ": DEFAULT_SOULSEEK_DIR,
        "Splits   ": DEFAULT_SPLITS_DIR,
        "Cache    ": CACHE_DIR,
        "Beets cfg": BEETS_CONFIG.parent,
    }
    for label, d in dirs.items():
        existed = d.exists()
        d.mkdir(parents=True, exist_ok=True)
        status = "[dim]already exists[/dim]" if existed else "[green]created[/green]"
        console.print(f"  {label}  {d}  {status}")


def _configure_beets() -> None:
    """Ensure BEETS_CONFIG contains muzik's required import defaults.

    • If the config file doesn't exist yet, a minimal one is created.
    • If it already has the required import settings, nothing is changed.
    • If it has an ``import:`` section, any missing required keys are inserted
      right after ``import:``.
    • Otherwise the full ``import:`` block is appended.
    """
    cfg = BEETS_CONFIG

    if not cfg.exists():
        cfg.write_text(
            "# beets config — created by muzik init\n"
            "# See https://beets.readthedocs.io/en/stable/reference/config.html\n\n"
            + _PLUGINS_BLOCK
            + "\n"
            + _IMPORT_BLOCK
        )
        console.print(f"  Beets cfg  {cfg}  [green]created[/green]")
        return

    text = cfg.read_text()
    changed: list[str] = []

    has_import_defaults = (
        "move:" in text and "duplicate_action" in text and "none_rec_action" in text
    )
    if not has_import_defaults:
        text = _add_import_defaults(text)
        changed.append("import defaults")

    # Album-art plugins. Only append when there is no plugins line to merge into.
    if "fetchart" not in text or "embedart" not in text:
        if re.search(r"^plugins\s*:", text, re.MULTILINE):
            console.print(
                "  [yellow]Add 'fetchart embedart' to your beets plugins line "
                "for album art.[/yellow]"
            )
        else:
            text = text.rstrip("\n") + "\n\n" + _PLUGINS_BLOCK
            changed.append("art plugins")

    if not changed:
        console.print(f"  Beets cfg  {cfg}  [dim]already set — skipped[/dim]")
        return
    cfg.write_text(text)
    console.print(f"  Beets cfg  {cfg}  [green]added {', '.join(changed)}[/green]")


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
    """Set up muzik: create app directories and configure beets.

    \b
    Creates:
      platform user data dir/downloads   — default download directory
      platform user data dir/soulseek    — default Soulseek download directory
      platform user data dir/splits      — default splits directory
      platform user cache dir            — cache directory
      platform beets config dir          — beets config directory

    \b
    Beets config changes:
      Sets import.move = yes so imports move files into the beets library.
      Sets import.duplicate_action = skip so that albums already present in
      the library are silently skipped on every workflow re-run.
      Existing settings are preserved; the file is only written if the
      setting is missing.
    """
    console.print("[bold]Directories[/bold]")
    _ensure_dirs()

    console.print("\n[bold]Beets configuration[/bold]")
    _configure_beets()

    console.print("\n[bold]Soulseek configuration[/bold]")
    console.print(f"  SLSKD_URL           [dim]{SLSKD_URL}[/dim]")
    console.print(f"  SLSKD_DOWNLOAD_DIR  [dim]{SLSKD_DOWNLOAD_DIR}[/dim]")
    if SLSKD_API_KEY:
        console.print("  SLSKD_API_KEY       [green]set[/green]")
    else:
        console.print("  SLSKD_API_KEY       [yellow]not set[/yellow]")
        console.print(
            "  [dim]Set SLSKD_URL, SLSKD_API_KEY, and SLSKD_DOWNLOAD_DIR "
            "to use Soulseek via slskd.[/dim]"
        )

    console.rule()
    console.print("[bold green]muzik init complete.[/bold green]")
    console.print(
        "\n[dim]Run [bold]muzik workflow <url>[/bold] to start downloading.[/dim]"
    )

"""muzik bandcamp — Bandcamp collection downloader + beets organize."""

import webbrowser
from pathlib import Path
from typing import Optional

import typer

from muzik.commands.organize import organize_cmd
from muzik.config import BEETS_CONFIG, DEFAULT_BANDCAMP_DIR, MUZIK_CONFIG_DIR
from muzik.core.bandcamp import run as bc_run
from muzik.ui.console import console, err

_COOKIES_TXT = MUZIK_CONFIG_DIR / "bandcamp_cookies.txt"
_USER_FILE = MUZIK_CONFIG_DIR / "bandcamp_user"

_FORMATS = "flac, wav, aac-hi, mp3-320, aiff-lossless, vorbis, mp3-v0, alac"

_SETUP_INSTRUCTIONS = """\
[bold]Bandcamp login[/bold]

  A browser window will open — log in to Bandcamp, then come back here.
  Your session cookies will be captured automatically.

Credentials will be stored in [dim]{dir}[/dim]
"""


# ---------------------------------------------------------------------------
# Credential management
# ---------------------------------------------------------------------------


def _stored_credentials() -> tuple[Optional[Path], Optional[str]]:
    cookies = _COOKIES_TXT if _COOKIES_TXT.exists() else None
    user = _USER_FILE.read_text().strip() if _USER_FILE.exists() else None
    return cookies, user or None


def _playwright_setup() -> tuple[Path, str]:
    """Open a headed browser, wait for Bandcamp login, extract cookies + username."""
    try:
        from playwright.sync_api import sync_playwright
    except ImportError:
        err(
            "[red]playwright not installed.[/red] Run: "
            "[bold]uv add playwright && uv run playwright install chromium[/bold]"
        )
        raise typer.Exit(1)

    from muzik.core.bandcamp import write_netscape_cookies

    console.print(_SETUP_INSTRUCTIONS.format(dir=MUZIK_CONFIG_DIR))
    webbrowser.open("https://bandcamp.com/login")

    username: Optional[str] = None

    with sync_playwright() as p:
        browser = p.chromium.launch(headless=False)
        context = browser.new_context()
        page = context.new_page()

        page.goto("https://bandcamp.com/login")
        console.print("[dim]Waiting for login…[/dim]")
        page.wait_for_url(
            lambda url: "bandcamp.com/login" not in url,
            timeout=300_000,
        )
        console.print("[green]Login detected![/green]")

        try:
            api_page = context.new_page()
            resp = api_page.goto("https://bandcamp.com/api/fan/2/collection_summary")
            if resp and resp.ok:
                data = resp.json()
                username = (
                    data.get("fan_data", {}).get("username")
                    or data.get("username")
                )
            api_page.close()
        except Exception:
            pass

        raw_cookies = context.cookies(["https://bandcamp.com"])
        browser.close()

    if not username:
        username = typer.prompt(
            "Could not detect username automatically.\nYour Bandcamp username"
        ).strip()
        while not username:
            err("[red]Username cannot be empty.[/red]")
            username = typer.prompt("Your Bandcamp username").strip()

    MUZIK_CONFIG_DIR.mkdir(parents=True, exist_ok=True)
    write_netscape_cookies(raw_cookies, _COOKIES_TXT)
    _USER_FILE.write_text(username)

    console.print(f"[green]Cookies saved →[/green] {_COOKIES_TXT}")
    console.print(f"[green]Username saved →[/green] {username}")
    return _COOKIES_TXT, username


def _ensure_credentials(
    explicit_cookies: Optional[Path],
    explicit_user: Optional[str],
    setup: bool,
) -> tuple[Path, str]:
    if setup:
        return _playwright_setup()

    stored_cookies, stored_user = _stored_credentials()
    cookies = explicit_cookies or stored_cookies
    user = explicit_user or stored_user

    if cookies and user:
        if not explicit_cookies:
            console.print(f"[dim]Using stored cookies: {cookies}[/dim]")
        return cookies, user

    missing = []
    if not cookies:
        missing.append("cookies")
    if not user:
        missing.append("username")
    console.print(
        f"[yellow]Bandcamp {' and '.join(missing)} not found — running setup.[/yellow]"
    )
    return _playwright_setup()


# ---------------------------------------------------------------------------
# Command
# ---------------------------------------------------------------------------


def bandcamp_cmd(
    user: Optional[str] = typer.Argument(None, help="Bandcamp username (saved after first run)."),
    output: Path = typer.Option(
        DEFAULT_BANDCAMP_DIR,
        "--output",
        "-o",
        help="Directory to save downloaded files.",
    ),
    format: str = typer.Option(  # noqa: A002
        "flac",
        "--format",
        "-f",
        help=f"Audio format ({_FORMATS}).",
    ),
    cookies: Optional[Path] = typer.Option(
        None,
        "--cookies",
        "-c",
        help="Path to cookies file (overrides stored credentials).",
    ),
    setup: bool = typer.Option(
        False,
        "--setup",
        help="Re-run the login flow (e.g. after cookies expire).",
    ),
    jobs: int = typer.Option(
        4,
        "--jobs",
        "-j",
        help="Number of parallel download threads.",
    ),
    dry_run: bool = typer.Option(
        False,
        "--dry-run",
        "-d",
        help="List releases without downloading.",
    ),
    force: bool = typer.Option(
        False,
        "--force",
        "-F",
        help="Bypass download cache and re-download everything.",
    ),
    no_organize: bool = typer.Option(
        False,
        "--no-organize",
        help="Skip beets organization after downloading.",
    ),
    import_: bool = typer.Option(
        False,
        "--import",
        "-i",
        help="Import files into beets library (moves files).",
    ),
    tag_only: bool = typer.Option(
        False,
        "--tag-only",
        "-t",
        help="Only write tags with beets; do not move files.",
    ),
    beets_config: Optional[Path] = typer.Option(
        None,
        "--beets-config",
        help=f"Beets config file (default: {BEETS_CONFIG}).",
    ),
) -> None:
    """Download your Bandcamp collection, then organize with beets.

    On first run, opens a browser window for you to log in to Bandcamp.
    Cookies and username are captured automatically and stored in
    $XDG_CONFIG_HOME/muzik/ — subsequent runs need no arguments at all.

    Re-run login after cookies expire: [bold]muzik bandcamp --setup[/bold]
    """
    cookies_path, username = _ensure_credentials(cookies, user, setup)

    if setup and user is None:
        raise typer.Exit(0)

    console.print(f"[bold cyan]Bandcamp download:[/bold cyan] {username}")
    console.print(f"[dim]Format: {format} · Output: {output.resolve()}[/dim]")

    before = {d for d in output.iterdir() if d.is_dir()} if output.exists() else set()

    bc_run(
        username=username,
        cookies_path=cookies_path,
        output=output,
        audio_format=format,
        jobs=jobs,
        force=force,
        dry_run=dry_run,
    )

    if no_organize or dry_run:
        return

    after_dirs = {d for d in output.iterdir() if d.is_dir()}
    new_dirs = sorted(after_dirs - before)

    if not new_dirs:
        new_dirs = [output]

    console.print(f"\n[bold]Organize[/bold] — {len(new_dirs)} director(ies) via beets")
    for d in new_dirs:
        console.print(f"  beet import [dim]{d}[/dim]")
        try:
            organize_cmd(
                directory=d,
                import_=import_,
                tag_only=tag_only,
                dry_run=False,
                config=beets_config,
            )
        except (SystemExit, typer.Exit) as exc:
            if getattr(exc, "code", 0) != 0:
                err(f"  [red]beet failed for {d.name}[/red]")

    console.print("[bold green]Bandcamp workflow complete.[/bold green]")

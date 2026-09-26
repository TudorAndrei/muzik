"""muzik spotify [login|logout|status|playlists|export|watch] — Spotify Web API."""

from __future__ import annotations

import json
from pathlib import Path

import typer
from rich.table import Table

from muzik.config import get_spotify_settings, save_muzik_config_value
from muzik.core.sources.spotify_api import (
    LIKED_URI,
    SpotifyApiError,
    SpotifyClient,
    export_document,
)
from muzik.core.sources.spotify_auth import (
    SpotifyAuthError,
    TokenStore,
    login as run_login,
    redirect_uri,
)
from muzik.core.watchlist import WatchlistError, WatchlistRepository
from muzik.ui.console import console


app = typer.Typer(help="Connect Spotify and read playlists with its Web API.")

_SETUP_HELP = (
    "Create an application at https://developer.spotify.com/dashboard, add "
    "the redirect URI that 'muzik spotify status' shows, then run "
    "'muzik spotify set-client-id <id>'."
)


def _client() -> SpotifyClient:
    return SpotifyClient()


@app.command("set-client-id")
def set_client_id(
    client_id: str = typer.Argument(..., help="Client ID of your Spotify application."),
) -> None:
    """Save the client ID of your own Spotify application."""
    value = client_id.strip()
    if not value:
        console.print("[red]Enter the client ID.[/red]")
        raise typer.Exit(code=1)
    save_muzik_config_value("spotify", "client_id", value)
    console.print("[green]Spotify client ID saved.[/green]")


@app.command("status")
def status() -> None:
    """Show the Spotify client ID, the redirect URI, and the connection."""
    settings = get_spotify_settings()
    port = settings.get("redirect_port", "8888")
    console.print(
        f"Client ID: {settings.get('client_id') or '[yellow]not set[/yellow]'}"
    )
    console.print(f"Redirect URI: {redirect_uri(int(port))}")
    console.print(
        "[dim]Add that URI to the Redirect URIs of your application, exactly "
        "as it is written here. Spotify compares the two values character for "
        "character, thus 'localhost' does not match '127.0.0.1'.[/dim]"
    )
    tokens = TokenStore().load()
    if tokens is None:
        console.print("Connection: [yellow]not connected[/yellow]")
        console.print(_SETUP_HELP)
        return
    try:
        account = _client().account_name()
    except (SpotifyApiError, SpotifyAuthError) as exc:
        console.print(f"Connection: [red]{exc}[/red]")
        raise typer.Exit(code=1) from exc
    console.print(f"Connection: [green]connected as {account}[/green]")


@app.command("login")
def login(
    port: int = typer.Option(
        None,
        "--port",
        "-p",
        help="Loopback port for the redirect. It is saved for the next login.",
    ),
) -> None:
    """Connect muzik to your Spotify account in a browser."""
    if port is not None:
        save_muzik_config_value("spotify", "redirect_port", str(port))
    settings = get_spotify_settings()
    uri = redirect_uri(int(settings.get("redirect_port", "8888") or "8888"))
    console.print(f"Your Spotify application must have this redirect URI: {uri}")
    console.print("Opening the browser for the Spotify login...")
    try:
        run_login()
        account = _client().account_name()
    except (SpotifyApiError, SpotifyAuthError) as exc:
        console.print(f"[red]{exc}[/red]")
        raise typer.Exit(code=1) from exc
    console.print(f"[green]Connected as {account}.[/green]")


@app.command("logout")
def logout() -> None:
    """Remove the saved Spotify tokens."""
    if TokenStore().clear():
        console.print("[green]Spotify tokens removed.[/green]")
        return
    console.print("No Spotify tokens were saved.")


@app.command("playlists")
def playlists() -> None:
    """List Liked Songs and every playlist of the connected account."""
    try:
        references = _client().list_playlists()
    except (SpotifyApiError, SpotifyAuthError) as exc:
        console.print(f"[red]{exc}[/red]")
        raise typer.Exit(code=1) from exc
    table = Table(title="Spotify playlists")
    table.add_column("Name")
    table.add_column("Tracks", justify="right")
    table.add_column("Reference")
    for reference in references:
        table.add_row(
            reference.name,
            "" if reference.total is None else str(reference.total),
            reference.uri,
        )
    console.print(table)


@app.command("export")
def export(
    reference: str = typer.Argument(
        ...,
        help="A playlist link, 'spotify:playlist:<id>', or 'liked'.",
    ),
    output: Path = typer.Option(
        None,
        "--output",
        "-o",
        help="Write the export to this file instead of the screen.",
    ),
) -> None:
    """Write one playlist as a canonical Spotify export document."""
    uri = (
        LIKED_URI
        if reference.strip().lower() in {"liked", "liked songs"}
        else reference
    )
    try:
        playlist = _client().load_playlist(uri)
    except (SpotifyApiError, SpotifyAuthError) as exc:
        console.print(f"[red]{exc}[/red]")
        raise typer.Exit(code=1) from exc
    document = json.dumps(export_document(playlist), indent=2, ensure_ascii=False)
    if output is None:
        console.print_json(document)
        return
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(document + "\n", encoding="utf-8")
    console.print(f"[green]Wrote {len(playlist.entries)} track(s) to {output}.[/green]")


@app.command("watch")
def watch(
    reference: str = typer.Argument(
        ...,
        help="A playlist link, 'spotify:playlist:<id>', or 'liked'.",
    ),
) -> None:
    """Add one Spotify playlist to the watchlist."""
    try:
        playlist = WatchlistRepository().add(reference)
    except WatchlistError as exc:
        console.print(f"[red]{exc}[/red]")
        raise typer.Exit(code=1) from exc
    console.print(
        f"[green]Added {playlist.display_name}.[/green] "
        "Open the Watchlist tab, or run a refresh, to sync it."
    )

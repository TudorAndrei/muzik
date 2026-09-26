"""Spotify OAuth with Authorization Code and PKCE.

muzik ships no client secret. The user registers one application in the
Spotify developer dashboard and gives muzik its client ID. The browser sends
the authorization code back to a loopback address that only listens for one
request.
"""

from __future__ import annotations

import base64
from collections.abc import Callable, Mapping
from dataclasses import dataclass
import hashlib
import http.server
import json
import os
from pathlib import Path
import secrets
import socket
import threading
import time
from typing import Any, cast
import urllib.error
import urllib.parse
import urllib.request
import webbrowser

from muzik.config import MUZIK_SPOTIFY_TOKEN_FILE, get_spotify_settings


AUTHORIZE_URL = "https://accounts.spotify.com/authorize"
TOKEN_URL = "https://accounts.spotify.com/api/token"
SCOPES = (
    "playlist-read-private",
    "playlist-read-collaborative",
    "user-library-read",
)
# The access token is refreshed this many seconds before it expires, so that
# a long page of results cannot expire in the middle of a request.
_EXPIRY_MARGIN = 60.0
_LOGIN_TIMEOUT = 300.0
_SUCCESS_PAGE = (
    b"<html><body style='font-family:sans-serif'>"
    b"<h3>muzik is connected to Spotify.</h3>"
    b"<p>You can close this tab.</p></body></html>"
)


class SpotifyAuthError(RuntimeError):
    """Raised when Spotify authorization or a token refresh fails."""


@dataclass(frozen=True, slots=True)
class SpotifyTokens:
    """One saved Spotify session."""

    access_token: str
    refresh_token: str
    expires_at: float
    scope: str = ""

    def is_expired(self, *, now: float | None = None) -> bool:
        current = time.time() if now is None else now
        return current >= self.expires_at - _EXPIRY_MARGIN

    def to_dict(self) -> dict[str, Any]:
        return {
            "access_token": self.access_token,
            "refresh_token": self.refresh_token,
            "expires_at": self.expires_at,
            "scope": self.scope,
        }


class TokenStore:
    """Load and save the Spotify tokens of one user."""

    def __init__(self, path: Path | None = None) -> None:
        self.path = path if path is not None else MUZIK_SPOTIFY_TOKEN_FILE

    def load(self) -> SpotifyTokens | None:
        if not self.path.exists():
            return None
        try:
            raw = json.loads(self.path.read_text(encoding="utf-8"))
        except OSError, json.JSONDecodeError:
            return None
        if not isinstance(raw, dict):
            return None
        access = raw.get("access_token")
        refresh = raw.get("refresh_token")
        if not isinstance(access, str) or not isinstance(refresh, str):
            return None
        expires_at = raw.get("expires_at")
        return SpotifyTokens(
            access_token=access,
            refresh_token=refresh,
            expires_at=float(expires_at)
            if isinstance(expires_at, (int, float))
            else 0.0,
            scope=str(raw.get("scope") or ""),
        )

    def save(self, tokens: SpotifyTokens) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.path.write_text(
            json.dumps(tokens.to_dict(), indent=2) + "\n", encoding="utf-8"
        )
        os.chmod(self.path, 0o600)

    def clear(self) -> bool:
        if not self.path.exists():
            return False
        self.path.unlink()
        return True


HttpPost = Callable[[str, dict[str, str]], dict[str, Any]]


def post_form(url: str, data: dict[str, str]) -> dict[str, Any]:
    """Send one form POST and return the JSON body."""
    body = urllib.parse.urlencode(data).encode("utf-8")
    request = urllib.request.Request(
        url,
        data=body,
        headers={"Content-Type": "application/x-www-form-urlencoded"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            payload = json.loads(response.read().decode("utf-8"))
    except urllib.error.HTTPError as exc:
        raise SpotifyAuthError(_http_error_message(exc)) from exc
    except (urllib.error.URLError, TimeoutError) as exc:
        raise SpotifyAuthError(f"Unable to reach Spotify: {exc}") from exc
    except json.JSONDecodeError as exc:
        raise SpotifyAuthError("Spotify returned an invalid token response.") from exc
    if not isinstance(payload, dict):
        raise SpotifyAuthError("Spotify returned an invalid token response.")
    return cast(dict[str, Any], payload)


def _http_error_message(error: urllib.error.HTTPError) -> str:
    detail = ""
    try:
        body = json.loads(error.read().decode("utf-8"))
        if isinstance(body, dict):
            detail = str(
                body.get("error_description") or body.get("error") or ""
            ).strip()
    except OSError, ValueError:
        detail = ""
    if error.code == 400 and "redirect" in detail.lower():
        detail = (
            f"{detail}. Add the redirect URI to your Spotify application, "
            "exactly as muzik sends it."
        )
    return f"Spotify rejected the request ({error.code}): {detail or error.reason}"


def client_id(*, settings: Mapping[str, str] | None = None) -> str:
    """Return the configured client ID, or raise a usable message."""
    values = dict(settings) if settings is not None else get_spotify_settings()
    value = values.get("client_id", "").strip()
    if not value:
        raise SpotifyAuthError(
            "No Spotify client ID is configured. Create an application at "
            "https://developer.spotify.com/dashboard, then run "
            "'muzik spotify set-client-id <id>'."
        )
    return value


def redirect_uri(port: int) -> str:
    """Return the loopback redirect URI for *port*.

    Spotify accepts a loopback IP address, but not the name 'localhost'.
    """
    return f"http://127.0.0.1:{port}/callback"


def build_authorize_url(
    *,
    client_id_value: str,
    port: int,
    verifier: str,
    state: str,
    scopes: tuple[str, ...] = SCOPES,
) -> str:
    """Return the Spotify authorization URL for one PKCE login."""
    challenge = (
        base64.urlsafe_b64encode(hashlib.sha256(verifier.encode("ascii")).digest())
        .decode("ascii")
        .rstrip("=")
    )
    parameters = {
        "client_id": client_id_value,
        "response_type": "code",
        "redirect_uri": redirect_uri(port),
        "code_challenge_method": "S256",
        "code_challenge": challenge,
        "state": state,
        "scope": " ".join(scopes),
    }
    return f"{AUTHORIZE_URL}?{urllib.parse.urlencode(parameters)}"


def new_verifier() -> str:
    """Return one PKCE code verifier."""
    return secrets.token_urlsafe(64)[:128]


def tokens_from_payload(
    payload: Mapping[str, Any], *, fallback_refresh: str = ""
) -> SpotifyTokens:
    """Convert one Spotify token response into saved tokens."""
    access = payload.get("access_token")
    if not isinstance(access, str) or not access:
        raise SpotifyAuthError("Spotify returned no access token.")
    expires_in = payload.get("expires_in")
    lifetime = float(expires_in) if isinstance(expires_in, (int, float)) else 3600.0
    refresh = payload.get("refresh_token")
    refresh_value = (
        refresh if isinstance(refresh, str) and refresh else fallback_refresh
    )
    if not refresh_value:
        raise SpotifyAuthError("Spotify returned no refresh token.")
    return SpotifyTokens(
        access_token=access,
        refresh_token=refresh_value,
        expires_at=time.time() + lifetime,
        scope=str(payload.get("scope") or ""),
    )


class _CallbackHandler(http.server.BaseHTTPRequestHandler):
    """Accept the one redirect that carries the authorization code."""

    result: dict[str, str] = {}

    def do_GET(self) -> None:  # noqa: N802 - name required by the base class
        query = urllib.parse.urlparse(self.path).query
        values = urllib.parse.parse_qs(query)
        type(self).result = {key: value[0] for key, value in values.items() if value}
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.end_headers()
        self.wfile.write(_SUCCESS_PAGE)

    def log_message(self, format: str, *args: Any) -> None:  # noqa: A002
        """Keep the local server silent."""


def wait_for_code(port: int, *, timeout: float = _LOGIN_TIMEOUT) -> str:
    """Serve one loopback request and return its authorization code."""
    handler = type("MuzikSpotifyCallback", (_CallbackHandler,), {"result": {}})
    try:
        server = http.server.HTTPServer(("127.0.0.1", port), handler)
    except OSError as exc:
        raise SpotifyAuthError(
            f"Unable to listen on 127.0.0.1:{port} for the Spotify redirect: {exc}"
        ) from exc
    server.timeout = timeout
    thread = threading.Thread(target=server.handle_request, daemon=True)
    thread.start()
    thread.join(timeout)
    server.server_close()
    result = cast(dict[str, str], handler.result)
    if not result:
        raise SpotifyAuthError(
            "No Spotify answer was received. Your Spotify application must "
            f"have this exact redirect URI: {redirect_uri(port)} "
            "('localhost' does not match '127.0.0.1')."
        )
    if "error" in result:
        raise SpotifyAuthError(f"Spotify refused the login: {result['error']}")
    code = result.get("code", "")
    if not code:
        raise SpotifyAuthError("Spotify sent no authorization code.")
    return code


def check_port_free(port: int) -> None:
    """Raise when the loopback port for the redirect is already in use.

    This runs before the browser opens, thus the user reads one clear error
    instead of a Spotify page that cannot come back.
    """
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
        probe.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        try:
            probe.bind(("127.0.0.1", port))
        except OSError as exc:
            raise SpotifyAuthError(
                f"Port {port} is in use, thus muzik cannot receive the Spotify "
                "redirect. Stop that program, or set a different port with "
                "'muzik spotify login --port <number>'."
            ) from exc


def login(
    *,
    settings: Mapping[str, str] | None = None,
    store: TokenStore | None = None,
    opener: Callable[[str], Any] = webbrowser.open,
    poster: HttpPost = post_form,
    code_reader: Callable[[int], str] = wait_for_code,
    port_check: Callable[[int], None] = check_port_free,
) -> SpotifyTokens:
    """Run one PKCE login and save the tokens."""
    values = dict(settings) if settings is not None else get_spotify_settings()
    identifier = client_id(settings=values)
    port = int(values.get("redirect_port", "8888") or "8888")
    port_check(port)
    verifier = new_verifier()
    state = secrets.token_urlsafe(16)
    url = build_authorize_url(
        client_id_value=identifier,
        port=port,
        verifier=verifier,
        state=state,
    )
    opener(url)
    code = code_reader(port)
    payload = poster(
        TOKEN_URL,
        {
            "grant_type": "authorization_code",
            "code": code,
            "redirect_uri": redirect_uri(port),
            "client_id": identifier,
            "code_verifier": verifier,
        },
    )
    tokens = tokens_from_payload(payload)
    (store or TokenStore()).save(tokens)
    return tokens


def refresh_tokens(
    tokens: SpotifyTokens,
    *,
    settings: Mapping[str, str] | None = None,
    store: TokenStore | None = None,
    poster: HttpPost = post_form,
) -> SpotifyTokens:
    """Exchange the refresh token for a new access token and save it."""
    values = dict(settings) if settings is not None else get_spotify_settings()
    identifier = client_id(settings=values)
    payload = poster(
        TOKEN_URL,
        {
            "grant_type": "refresh_token",
            "refresh_token": tokens.refresh_token,
            "client_id": identifier,
        },
    )
    refreshed = tokens_from_payload(payload, fallback_refresh=tokens.refresh_token)
    (store or TokenStore()).save(refreshed)
    return refreshed


def current_tokens(
    *,
    settings: Mapping[str, str] | None = None,
    store: TokenStore | None = None,
    poster: HttpPost = post_form,
) -> SpotifyTokens:
    """Return usable tokens, refreshed if the access token has expired."""
    token_store = store or TokenStore()
    tokens = token_store.load()
    if tokens is None:
        raise SpotifyAuthError(
            "muzik is not connected to Spotify. Run 'muzik spotify login'."
        )
    if not tokens.is_expired():
        return tokens
    return refresh_tokens(tokens, settings=settings, store=token_store, poster=poster)


def is_connected(*, store: TokenStore | None = None) -> bool:
    """Return whether saved Spotify tokens exist."""
    return (store or TokenStore()).load() is not None

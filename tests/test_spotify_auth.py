import base64
import hashlib
import json
from pathlib import Path
import time
from urllib.parse import parse_qs, urlsplit

import pytest

from muzik.core.sources.spotify_auth import (
    SpotifyAuthError,
    SpotifyTokens,
    TokenStore,
    build_authorize_url,
    check_port_free,
    client_id,
    current_tokens,
    login,
    redirect_uri,
    refresh_tokens,
    tokens_from_payload,
)


SETTINGS = {"client_id": "client-1", "redirect_port": "8888"}


def _store(tmp_path: Path) -> TokenStore:
    return TokenStore(tmp_path / "spotify-token.json")


def test_authorize_url_carries_the_pkce_challenge_and_loopback_redirect() -> None:
    verifier = "verifier-value"

    url = build_authorize_url(
        client_id_value="client-1",
        port=8888,
        verifier=verifier,
        state="state-1",
    )

    query = parse_qs(urlsplit(url).query)
    expected = (
        base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest())
        .decode()
        .rstrip("=")
    )
    assert query["code_challenge"] == [expected]
    assert query["code_challenge_method"] == ["S256"]
    assert query["response_type"] == ["code"]
    assert query["redirect_uri"] == ["http://127.0.0.1:8888/callback"]
    assert "user-library-read" in query["scope"][0]
    assert "=" not in query["code_challenge"][0]


def test_redirect_uri_uses_the_loopback_address_not_localhost() -> None:
    assert redirect_uri(9000) == "http://127.0.0.1:9000/callback"


def test_missing_client_id_explains_how_to_get_one() -> None:
    with pytest.raises(SpotifyAuthError, match="developer.spotify.com"):
        client_id(settings={"client_id": ""})


def test_login_exchanges_the_code_and_saves_the_tokens(tmp_path: Path) -> None:
    store = _store(tmp_path)
    opened: list[str] = []
    posted: list[dict[str, str]] = []

    def poster(url: str, data: dict[str, str]) -> dict[str, object]:
        posted.append(data)
        return {
            "access_token": "access-1",
            "refresh_token": "refresh-1",
            "expires_in": 3600,
            "scope": "user-library-read",
        }

    tokens = login(
        settings=SETTINGS,
        store=store,
        opener=opened.append,
        poster=poster,
        code_reader=lambda port: "code-1",
        port_check=lambda port: None,
    )

    assert opened and opened[0].startswith("https://accounts.spotify.com/authorize?")
    assert posted[0]["grant_type"] == "authorization_code"
    assert posted[0]["code"] == "code-1"
    assert posted[0]["client_id"] == "client-1"
    assert posted[0]["code_verifier"]
    assert tokens.access_token == "access-1"
    assert store.load() == tokens


def test_saved_tokens_are_private_to_the_user(tmp_path: Path) -> None:
    store = _store(tmp_path)

    store.save(SpotifyTokens("access-1", "refresh-1", time.time() + 60))

    assert store.path.stat().st_mode & 0o077 == 0
    assert json.loads(store.path.read_text())["refresh_token"] == "refresh-1"
    assert store.clear() is True
    assert store.load() is None


def test_refresh_keeps_the_old_refresh_token_when_spotify_sends_none(
    tmp_path: Path,
) -> None:
    store = _store(tmp_path)
    tokens = SpotifyTokens("old", "refresh-1", 0.0)

    refreshed = refresh_tokens(
        tokens,
        settings=SETTINGS,
        store=store,
        poster=lambda url, data: {"access_token": "access-2", "expires_in": 3600},
    )

    assert refreshed.access_token == "access-2"
    assert refreshed.refresh_token == "refresh-1"
    assert store.load() == refreshed


def test_current_tokens_refreshes_only_an_expired_access_token(
    tmp_path: Path,
) -> None:
    store = _store(tmp_path)
    store.save(SpotifyTokens("access-1", "refresh-1", time.time() + 3600))
    calls: list[str] = []

    def poster(url: str, data: dict[str, str]) -> dict[str, object]:
        calls.append(data["grant_type"])
        return {"access_token": "access-2", "expires_in": 3600}

    fresh = current_tokens(settings=SETTINGS, store=store, poster=poster)
    store.save(SpotifyTokens("access-1", "refresh-1", time.time() - 1))
    stale = current_tokens(settings=SETTINGS, store=store, poster=poster)

    assert fresh.access_token == "access-1"
    assert stale.access_token == "access-2"
    assert calls == ["refresh_token"]


def test_current_tokens_without_a_login_says_what_to_run(tmp_path: Path) -> None:
    with pytest.raises(SpotifyAuthError, match="muzik spotify login"):
        current_tokens(settings=SETTINGS, store=_store(tmp_path))


def test_a_token_response_without_a_refresh_token_is_rejected() -> None:
    with pytest.raises(SpotifyAuthError, match="refresh token"):
        tokens_from_payload({"access_token": "access-1", "expires_in": 60})


def test_a_login_timeout_names_the_redirect_uri(tmp_path: Path) -> None:
    def timeout(port: int) -> str:
        raise SpotifyAuthError(
            "No Spotify answer was received. Your Spotify application must "
            f"have this exact redirect URI: {redirect_uri(port)}"
        )

    with pytest.raises(SpotifyAuthError, match="http://127.0.0.1:8888/callback"):
        login(
            settings=SETTINGS,
            store=_store(tmp_path),
            opener=lambda url: None,
            poster=lambda url, data: {},
            code_reader=timeout,
            port_check=lambda port: None,
        )


def test_a_busy_port_is_reported_before_the_browser_opens(tmp_path: Path) -> None:
    opened: list[str] = []

    def busy(port: int) -> None:
        raise SpotifyAuthError(f"Port {port} is in use")

    with pytest.raises(SpotifyAuthError, match="in use"):
        login(
            settings=SETTINGS,
            store=_store(tmp_path),
            opener=opened.append,
            poster=lambda url, data: {},
            code_reader=lambda port: "code-1",
            port_check=busy,
        )

    assert opened == []


def test_check_port_free_accepts_a_free_port_and_rejects_a_used_one() -> None:
    import socket

    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as taken:
        taken.bind(("127.0.0.1", 0))
        taken.listen(1)
        port = taken.getsockname()[1]

        with pytest.raises(SpotifyAuthError, match="in use"):
            check_port_free(port)

    check_port_free(port)

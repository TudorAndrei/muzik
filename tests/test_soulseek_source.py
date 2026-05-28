import json
from pathlib import Path

import pytest
import requests

from muzik.core import cache as cache_mod
from muzik.core.sources.base import ResolvedTrack
from muzik.core.sources.soulseek import (
    SoulseekError,
    SoulseekSource,
    candidate_from_response,
)


def test_candidate_from_response_scores_lossless_response() -> None:
    response = {
        "username": "peer",
        "token": "abc",
        "hasFreeUploadSlot": True,
        "queueLength": 0,
        "uploadSpeed": 500_000,
        "files": [
            {
                "filename": "Artist/Album/01 One.flac",
                "size": 123_000_000,
                "bitRate": None,
                "sampleRate": 44100,
                "bitDepth": 16,
                "length": 180,
            },
            {
                "filename": "Artist/Album/02 Two.flac",
                "size": 124_000_000,
                "sampleRate": 44100,
                "bitDepth": 16,
                "length": 190,
            },
        ],
    }

    candidate = candidate_from_response(
        response,
        query="Artist Album flac",
        expected_track_count=2,
    )

    assert candidate.source == "soulseek"
    assert candidate.user == "peer"
    assert candidate.quality.lossless is True
    assert candidate.files[0].quality.format == "flac"
    assert candidate.score > 100


def test_candidate_from_fixture_response() -> None:
    fixture = Path("tests/fixtures/slskd/search_response.json")
    response = json.loads(fixture.read_text(encoding="utf-8"))

    candidate = candidate_from_response(
        response,
        query="Artist Fixture Album flac",
        expected_track_count=2,
    )

    assert candidate.user == "fixture-peer"
    assert candidate.title == "Fixture Album"
    assert candidate.files[1].quality.format == "flac"
    assert candidate.score > 100


def test_soulseek_download_writes_metadata_and_cache(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")
    audio = tmp_path / "01 One.flac"
    audio.write_bytes(b"")

    candidate = candidate_from_response(
        {
            "username": "peer",
            "token": "abc",
            "files": [{"filename": "01 One.flac", "size": 1}],
        },
        query="Artist Album flac",
    )

    class Transfers:
        def enqueue(self, username, files):
            assert username == "peer"
            assert files[0]["filename"] == "01 One.flac"
            return True

    class Client:
        transfers = Transfers()

    source = SoulseekSource(api_key="key", download_dir=tmp_path)
    source._client = Client()

    result = source.download(candidate, wait=False, verify=False)

    assert result.files == [audio]
    assert result.metadata_path == tmp_path / "01 One.muzik.json"
    assert result.metadata_path is not None
    assert result.metadata_path.exists()
    cache_key = cache_mod.download_cache_key("soulseek", candidate.source_id)
    assert cache_mod.get(cache_key) == str(audio.resolve())


def test_soulseek_source_requires_api_key() -> None:
    source = SoulseekSource(api_key="")

    with pytest.raises(SoulseekError, match="SLSKD_API_KEY"):
        _ = source.client


def test_soulseek_download_verifies_files_with_ffprobe(
    tmp_path: Path,
    monkeypatch,
) -> None:
    audio = tmp_path / "01 One.flac"
    audio.write_bytes(b"")
    candidate = candidate_from_response(
        {
            "username": "peer",
            "token": "abc",
            "files": [{"filename": "01 One.flac", "size": 1}],
        },
        query="Artist Album flac",
    )

    class Transfers:
        def enqueue(self, username, files):
            return True

    class Client:
        transfers = Transfers()

    source = SoulseekSource(api_key="key", download_dir=tmp_path)
    source._client = Client()
    monkeypatch.setattr("muzik.core.sources.soulseek.get_duration", lambda path: None)

    with pytest.raises(SoulseekError, match="ffprobe"):
        source.download(candidate, wait=False, verify=True)


def test_wait_for_candidate_fails_when_queued_too_long(monkeypatch) -> None:
    candidate = candidate_from_response(
        {
            "username": "peer",
            "token": "abc",
            "files": [{"filename": "01 One.flac", "size": 1}],
        },
        query="Artist Album flac",
    )

    class Transfers:
        def get_downloads(self, username):
            return {
                "directories": [
                    {
                        "files": [
                            {
                                "filename": "01 One.flac",
                                "state": "Queued",
                            }
                        ]
                    }
                ]
            }

    class Client:
        transfers = Transfers()

    source = SoulseekSource(api_key="key")
    source._client = Client()
    monkeypatch.setattr("muzik.core.sources.soulseek.time.sleep", lambda seconds: None)
    monkeypatch.setattr("muzik.core.sources.soulseek.time.monotonic", lambda: 0)

    with pytest.raises(SoulseekError, match="queued"):
        source._wait_for_candidate(candidate, timeout=10, queue_timeout=0)


def test_soulseek_search_deletes_completed_conflicting_search() -> None:
    calls = {"search": 0, "delete": 0}

    class Searches:
        def search_text(self, query, *, responseLimit, searchTimeout):
            calls["search"] += 1
            if calls["search"] == 1:
                response = requests.Response()
                response.status_code = 409
                raise requests.HTTPError(response=response)
            return {"id": "new-search"}

        def get_all(self):
            return [
                {
                    "id": "old-search",
                    "searchText": "Artist Title flac",
                    "state": "Completed, Errored",
                    "isComplete": True,
                }
            ]

        def delete(self, search_id):
            assert search_id == "old-search"
            calls["delete"] += 1
            return True

        def state(self, search_id, *, includeResponses):
            assert search_id == "new-search"
            assert includeResponses is True
            return {"isComplete": True, "responses": []}

        def search_responses(self, search_id):
            assert search_id == "new-search"
            return []

    class Client:
        searches = Searches()

        class Session:
            def auth_valid(self):
                return True

        class Application:
            def state(self):
                return {
                    "server": {
                        "state": "Connected",
                        "isConnected": True,
                        "isLoggedIn": True,
                    }
                }

        session = Session()
        application = Application()

    source = SoulseekSource(api_key="key")
    source._client = Client()

    results = source.search(
        ResolvedTrack(title="Title", artist="Artist"),
        prefer="flac",
    )

    assert results == []
    assert calls == {"search": 2, "delete": 1}


def test_soulseek_search_fails_fast_when_slskd_is_not_logged_in() -> None:
    class Client:
        class Session:
            def auth_valid(self):
                return True

        class Application:
            def state(self):
                return {
                    "server": {
                        "state": "None",
                        "isConnected": False,
                        "isLoggedIn": False,
                    }
                }

        session = Session()
        application = Application()

    source = SoulseekSource(api_key="key")
    source._client = Client()

    with pytest.raises(SoulseekError, match="not connected and logged in"):
        source.search(ResolvedTrack(title="Title", artist="Artist"), prefer="flac")


def test_soulseek_search_reads_embedded_state_responses() -> None:
    class Searches:
        def search_text(self, query, *, responseLimit, searchTimeout):
            assert query == "mozart flac"
            return {"id": "search-id"}

        def state(self, search_id, *, includeResponses):
            assert search_id == "search-id"
            assert includeResponses is True
            return {
                "isComplete": True,
                "responses": [
                    {
                        "username": "peer",
                        "token": "abc",
                        "files": [
                            {
                                "filename": "Mozart/Album/01 One.flac",
                                "size": 1,
                            }
                        ],
                    }
                ],
            }

        def search_responses(self, search_id):
            raise AssertionError("embedded state responses should be used")

    class Client:
        class Session:
            def auth_valid(self):
                return True

        class Application:
            def state(self):
                return {
                    "server": {
                        "state": "Connected, LoggedIn",
                        "isConnected": True,
                        "isLoggedIn": True,
                    }
                }

        searches = Searches()
        session = Session()
        application = Application()

    source = SoulseekSource(api_key="key")
    source._client = Client()

    results = source.search(ResolvedTrack(title="mozart flac"), prefer="flac")

    assert len(results) == 1
    assert results[0].user == "peer"

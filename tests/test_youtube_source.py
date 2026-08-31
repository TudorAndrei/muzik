from pathlib import Path

import pytest

from muzik.core.sources import youtube
from muzik.core.sources.base import DownloadRequest, ResolvedPlaylist, ResolvedTrack
from muzik.core.sources.youtube import YouTubeSource


class Result:
    def __init__(self, returncode: int, stdout: str = "", stderr: str = "") -> None:
        self.returncode = returncode
        self.stdout = stdout
        self.stderr = stderr


def test_youtube_id_and_playlist_id_parsing() -> None:
    assert youtube.youtube_id("https://youtu.be/abcdefghijk") == "abcdefghijk"
    assert (
        youtube.youtube_id("https://youtube.com/watch?v=abcdefghijk") == "abcdefghijk"
    )
    assert youtube.playlist_id("https://youtube.com/watch?v=x&list=PL123") == "PL123"
    assert youtube.video_id_from_path(Path("Title [abcdefghijk].flac")) == "abcdefghijk"


def test_build_download_command_includes_expected_flags() -> None:
    cmd = youtube.build_download_command(
        "https://youtube.com/watch?v=abcdefghijk",
        archive_file=Path("archive.txt"),
    )

    assert cmd[0] == "yt-dlp"
    assert "--extract-audio" in cmd
    assert "--write-info-json" in cmd
    assert "--download-archive" in cmd
    assert "--force-overwrites" not in cmd
    assert "--write-thumbnail" in cmd
    assert cmd[cmd.index("--convert-thumbnails") + 1] == "jpg"
    assert cmd[-1] == "https://youtube.com/watch?v=abcdefghijk"


def test_js_runtime_args_prefers_available_runtime(monkeypatch) -> None:
    monkeypatch.setattr(youtube.shutil, "which", lambda name: name == "node")
    assert youtube.js_runtime_args() == ["--js-runtimes", "node"]

    monkeypatch.setattr(youtube.shutil, "which", lambda name: False)
    assert youtube.js_runtime_args() == []


def test_build_download_command_enables_js_runtime(monkeypatch) -> None:
    monkeypatch.setattr(youtube.shutil, "which", lambda name: name == "bun")
    cmd = youtube.build_download_command("https://youtube.com/watch?v=abcdefghijk")
    assert cmd[cmd.index("--js-runtimes") + 1] == "bun"


def test_js_runtime_args_does_not_use_deno(monkeypatch) -> None:
    monkeypatch.setattr(youtube.shutil, "which", lambda name: name == "deno")

    assert youtube.js_runtime_args() == []


def test_cookie_args_from_env(monkeypatch) -> None:
    monkeypatch.delenv("MUZIK_YTDLP_COOKIES", raising=False)
    monkeypatch.delenv("MUZIK_YTDLP_COOKIES_FROM_BROWSER", raising=False)
    assert youtube.cookie_args() == []

    monkeypatch.setenv("MUZIK_YTDLP_COOKIES_FROM_BROWSER", "chrome")
    assert youtube.cookie_args() == ["--cookies-from-browser", "chrome"]

    monkeypatch.delenv("MUZIK_YTDLP_COOKIES_FROM_BROWSER", raising=False)
    monkeypatch.setenv("MUZIK_YTDLP_COOKIES", "/tmp/cookies.txt")
    assert youtube.cookie_args() == ["--cookies", "/tmp/cookies.txt"]


def test_build_download_command_includes_cookies_when_set(monkeypatch) -> None:
    monkeypatch.setenv("MUZIK_YTDLP_COOKIES_FROM_BROWSER", "firefox")
    cmd = youtube.build_download_command("https://youtube.com/watch?v=abcdefghijk")
    assert cmd[1:3] == ["--cookies-from-browser", "firefox"]


def test_build_download_command_force_adds_force_overwrites() -> None:
    cmd = youtube.build_download_command(
        "https://youtube.com/watch?v=abcdefghijk",
        force=True,
    )

    assert "--force-overwrites" in cmd


def test_get_playlist_video_ids_uses_yt_dlp_flat_playlist(monkeypatch) -> None:
    seen = {}

    def fake_run_silent(cmd):
        seen["cmd"] = cmd
        return Result(0, "one\ntwo\n")

    monkeypatch.setattr(youtube, "run_silent", fake_run_silent)

    assert youtube.get_playlist_video_ids("https://youtube.com/playlist?list=PL") == [
        "one",
        "two",
    ]
    cmd = seen["cmd"]
    assert cmd[0] == "yt-dlp"
    assert "--flat-playlist" in cmd and "--print" in cmd
    assert cmd[-1] == "https://youtube.com/playlist?list=PL"


def test_get_playlist_items_returns_ordered_metadata(monkeypatch) -> None:
    seen = {}

    def fake_run_silent(cmd):
        seen["cmd"] = cmd
        return Result(
            0,
            """{
              "entries": [
                {
                  "playlist_index": 4,
                  "id": "abcdefghijk",
                  "title": "A long mix",
                  "thumbnail": "https://img.test/a.jpg"
                },
                {
                  "id": "lmnopqrstuv",
                  "title": "Second mix",
                  "webpage_url": "https://youtube.test/watch?v=lmnopqrstuv",
                  "thumbnails": [{"url": "https://img.test/b.jpg"}]
                }
              ]
            }""",
        )

    monkeypatch.setattr(youtube, "run_silent", fake_run_silent)

    items = youtube.get_playlist_items("https://youtube.com/playlist?list=PL")

    assert items == [
        youtube.YouTubePlaylistItem(
            position=4,
            title="A long mix",
            video_id="abcdefghijk",
            video_url="https://www.youtube.com/watch?v=abcdefghijk",
            thumbnail_url="https://img.test/a.jpg",
        ),
        youtube.YouTubePlaylistItem(
            position=2,
            title="Second mix",
            video_id="lmnopqrstuv",
            video_url="https://youtube.test/watch?v=lmnopqrstuv",
            thumbnail_url="https://img.test/b.jpg",
        ),
    ]
    assert "--flat-playlist" in seen["cmd"]
    assert "--dump-single-json" in seen["cmd"]


def test_get_playlist_items_keeps_unavailable_entries(monkeypatch) -> None:
    monkeypatch.setattr(
        youtube,
        "run_silent",
        lambda cmd: Result(
            0,
            '{"entries": [{"playlist_index": 9}, null, '
            '{"id": "short", "title": "Private video"}]}',
        ),
    )

    items = youtube.get_playlist_items("https://youtube.com/playlist?list=PL")

    assert [item.position for item in items] == [9, 2, 3]
    assert [item.title for item in items] == [
        "Unavailable video",
        "Unavailable video",
        "Private video",
    ]
    assert all(item.video_id is None for item in items)


@pytest.mark.parametrize(
    ("result", "message"),
    [
        (Result(1, stderr="playlist unavailable"), "playlist unavailable"),
        (Result(0, "not-json"), "invalid playlist JSON"),
        (Result(0, "{}"), "no playlist entries"),
    ],
)
def test_get_playlist_items_reports_lookup_errors(
    monkeypatch,
    result: Result,
    message: str,
) -> None:
    monkeypatch.setattr(youtube, "run_silent", lambda cmd: result)

    with pytest.raises(youtube.PlaylistLookupError, match=message):
        youtube.get_playlist_items("https://youtube.com/playlist?list=PL")


def test_youtube_source_resolves_single_video_metadata(monkeypatch) -> None:
    def fake_dump_json(url: str, *, flat_playlist: bool = False):
        assert flat_playlist is False
        return {
            "id": "abcdefghijk",
            "title": "Artist - Title",
            "uploader": "Uploader",
            "upload_date": "20200102",
            "duration": 123,
        }

    monkeypatch.setattr(youtube, "dump_json", fake_dump_json)

    resolved = YouTubeSource().resolve(
        DownloadRequest(raw="https://youtube.com/watch?v=abcdefghijk", source="youtube")
    )

    assert isinstance(resolved, ResolvedTrack)
    assert resolved.source_id == "abcdefghijk"
    assert resolved.artist == "Uploader"
    assert resolved.year == "2020"


def test_youtube_source_resolves_playlist(monkeypatch) -> None:
    monkeypatch.setattr(youtube, "get_playlist_video_ids", lambda url: ["one", "two"])

    resolved = YouTubeSource().resolve(
        DownloadRequest(raw="https://youtube.com/playlist?list=PL123", source="youtube")
    )

    assert isinstance(resolved, ResolvedPlaylist)
    assert resolved.source_id == "PL123"
    assert [entry.source_id for entry in resolved.entries] == ["one", "two"]

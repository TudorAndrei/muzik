from pathlib import Path

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
    assert cmd[-1] == "https://youtube.com/watch?v=abcdefghijk"


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
    assert seen["cmd"][:3] == ["yt-dlp", "--flat-playlist", "--print"]


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

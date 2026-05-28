from pathlib import Path

from muzik.commands import download


def test_download_cmd_uses_ytdlp_and_reports_new_audio(
    tmp_path: Path,
    monkeypatch,
) -> None:
    seen_cmd: list[str] = []
    seen_cwd: Path | None = None
    seen_label = ""

    def fake_run_streaming(cmd, *, cwd, label):
        nonlocal seen_cmd, seen_cwd, seen_label
        seen_cmd = cmd
        seen_cwd = cwd
        seen_label = label
        (cwd / "Downloaded [abcdefghijk].flac").write_bytes(b"audio")
        return 0

    monkeypatch.setattr(download, "run_streaming", fake_run_streaming)
    monkeypatch.setattr(download, "find_chapters", lambda path: [])

    download.download_cmd(
        url="https://youtube.com/watch?v=abcdefghijk",
        output=tmp_path,
        format="bestaudio",
        quality="0",
        no_chapters=False,
        archive_file=None,
    )

    assert seen_cwd == tmp_path
    assert seen_label == "yt-dlp"
    assert "yt-dlp" == seen_cmd[0]
    assert "https://youtube.com/watch?v=abcdefghijk" == seen_cmd[-1]
    assert (tmp_path / "Downloaded [abcdefghijk].flac").exists()

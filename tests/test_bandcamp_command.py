from pathlib import Path

from muzik.commands import bandcamp


def test_bandcamp_dry_run_uses_downloader_without_organizing(
    tmp_path: Path,
    monkeypatch,
) -> None:
    calls: list[dict] = []
    cookies = tmp_path / "cookies.txt"
    cookies.write_text("# cookies\n", encoding="utf-8")

    def fake_bc_run(**kwargs):
        calls.append(kwargs)

    def fail_organize_cmd(**kwargs):
        raise AssertionError("dry-run Bandcamp command should not organize")

    monkeypatch.setattr(bandcamp, "bc_run", fake_bc_run)
    monkeypatch.setattr(bandcamp, "organize_cmd", fail_organize_cmd)

    bandcamp.bandcamp_cmd(
        user="fan",
        output=tmp_path / "bandcamp",
        format="flac",
        cookies=cookies,
        setup=False,
        jobs=2,
        dry_run=True,
        force=False,
        no_organize=False,
        import_=False,
        tag_only=False,
        beets_config=None,
    )

    assert calls == [
        {
            "username": "fan",
            "cookies_path": cookies,
            "output": tmp_path / "bandcamp",
            "audio_format": "flac",
            "jobs": 2,
            "force": False,
            "dry_run": True,
        }
    ]

from pathlib import Path

import pytest
import typer

from muzik.commands import organize


def test_organize_uses_internal_import_for_default_import(
    tmp_path: Path,
    monkeypatch,
) -> None:
    album = tmp_path / "Album"
    album.mkdir()
    config = tmp_path / "config.yaml"
    config.write_text("directory: /tmp/music\n", encoding="utf-8")
    calls: list[tuple] = []

    def fake_import_paths(options, *, decisions):
        calls.append((options, decisions))

    monkeypatch.setattr(organize, "import_paths", fake_import_paths)

    organize.organize_cmd(
        directory=album,
        import_=False,
        tag_only=False,
        dry_run=False,
        config=config,
    )

    assert len(calls) == 1
    options, decisions = calls[0]
    assert options.paths == [album]
    assert options.config_path == config
    assert options.move is True
    assert options.incremental is True
    assert options.dry_run is False


def test_organize_preserves_dry_run_in_internal_import(
    tmp_path: Path,
    monkeypatch,
) -> None:
    album = tmp_path / "Album"
    album.mkdir()
    calls = []

    monkeypatch.setattr(
        organize,
        "import_paths",
        lambda options, *, decisions: calls.append(options),
    )

    organize.organize_cmd(
        directory=album,
        import_=True,
        tag_only=False,
        dry_run=True,
        config=None,
    )

    assert calls[0].dry_run is True
    assert calls[0].move is True


def test_organize_uses_passthrough_for_tag_only(
    tmp_path: Path,
    monkeypatch,
) -> None:
    album = tmp_path / "Album"
    album.mkdir()
    calls: list[list[str]] = []

    def fake_run_passthrough(cmd):
        calls.append(cmd)
        return 0

    def fail_import_paths(*args, **kwargs):
        raise AssertionError("--tag-only should be isolated to passthrough")

    monkeypatch.setattr(organize, "run_passthrough", fake_run_passthrough)
    monkeypatch.setattr(organize, "import_paths", fail_import_paths)
    monkeypatch.setattr(organize, "_beet_bin", lambda: "beet")

    organize.organize_cmd(
        directory=album,
        import_=False,
        tag_only=True,
        dry_run=False,
        config=None,
    )

    assert calls == [["beet", "write", "--yes", str(album)]]


def test_organize_rejects_missing_directory(tmp_path: Path) -> None:
    with pytest.raises(typer.Exit) as exc:
        organize.organize_cmd(
            directory=tmp_path / "missing",
            import_=False,
            tag_only=False,
            dry_run=False,
            config=None,
        )

    assert exc.value.exit_code == 1


def test_organize_missing_config_uses_default_beets_config(
    tmp_path: Path,
    monkeypatch,
) -> None:
    album = tmp_path / "Album"
    album.mkdir()
    missing_config = tmp_path / "missing.yaml"
    calls = []

    monkeypatch.setattr(
        organize,
        "import_paths",
        lambda options, *, decisions: calls.append(options),
    )

    organize.organize_cmd(
        directory=album,
        import_=False,
        tag_only=False,
        dry_run=False,
        config=missing_config,
    )

    assert calls[0].config_path is None

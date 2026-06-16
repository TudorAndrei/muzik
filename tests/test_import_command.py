from pathlib import Path

import pytest
import typer

from muzik.commands import import_ as import_command


def test_import_uses_internal_import_with_default_move(
    tmp_path: Path,
    monkeypatch,
) -> None:
    library = tmp_path / "Library"
    library.mkdir()
    config = tmp_path / "config.yaml"
    config.write_text("directory: /tmp/music\n", encoding="utf-8")
    calls: list[tuple] = []

    def fake_import_paths(options, *, decisions):
        calls.append((options, decisions))

    monkeypatch.setattr(import_command, "import_paths", fake_import_paths)
    monkeypatch.setattr(import_command, "_notify", lambda directory: None)

    import_command.import_cmd(
        directory=library,
        copy=False,
        link=False,
        nowrite=False,
        quiet=False,
        dry_run=False,
        config=config,
    )

    assert len(calls) == 1
    options, decisions = calls[0]
    assert options.paths == [library]
    assert options.config_path == config
    assert options.copy is False
    assert options.link is False
    assert options.move is True
    assert options.nowrite is False
    assert options.quiet is False
    assert options.dry_run is False
    assert options.incremental is True
    assert decisions.quiet is False


def test_import_preserves_copy_mode(
    tmp_path: Path,
    monkeypatch,
) -> None:
    library = tmp_path / "Library"
    library.mkdir()
    calls = []

    monkeypatch.setattr(
        import_command,
        "import_paths",
        lambda options, *, decisions: calls.append(options),
    )
    monkeypatch.setattr(import_command, "_notify", lambda directory: None)

    import_command.import_cmd(
        directory=library,
        copy=True,
        link=False,
        nowrite=False,
        quiet=False,
        dry_run=False,
        config=None,
    )

    assert calls[0].copy is True
    assert calls[0].link is False
    assert calls[0].move is False


def test_import_preserves_link_mode(
    tmp_path: Path,
    monkeypatch,
) -> None:
    library = tmp_path / "Library"
    library.mkdir()
    calls = []

    monkeypatch.setattr(
        import_command,
        "import_paths",
        lambda options, *, decisions: calls.append(options),
    )
    monkeypatch.setattr(import_command, "_notify", lambda directory: None)

    import_command.import_cmd(
        directory=library,
        copy=False,
        link=True,
        nowrite=False,
        quiet=False,
        dry_run=False,
        config=None,
    )

    assert calls[0].copy is False
    assert calls[0].link is True
    assert calls[0].move is False


def test_import_preserves_nowrite_quiet_dry_run_and_config(
    tmp_path: Path,
    monkeypatch,
) -> None:
    library = tmp_path / "Library"
    library.mkdir()
    config = tmp_path / "config.yaml"
    config.write_text("directory: /tmp/music\n", encoding="utf-8")
    calls: list[tuple] = []
    notifications = []

    def fake_import_paths(options, *, decisions):
        calls.append((options, decisions))

    monkeypatch.setattr(import_command, "import_paths", fake_import_paths)
    monkeypatch.setattr(
        import_command,
        "_notify",
        lambda directory: notifications.append(directory),
    )

    import_command.import_cmd(
        directory=library,
        copy=False,
        link=False,
        nowrite=True,
        quiet=True,
        dry_run=True,
        config=config,
    )

    options, decisions = calls[0]
    assert options.config_path == config
    assert options.nowrite is True
    assert options.quiet is True
    assert options.dry_run is True
    assert decisions.quiet is True
    assert notifications == []


def test_import_notifies_for_non_quiet_internal_import(
    tmp_path: Path,
    monkeypatch,
) -> None:
    library = tmp_path / "Library"
    library.mkdir()
    notifications = []

    monkeypatch.setattr(
        import_command,
        "import_paths",
        lambda options, *, decisions: None,
    )
    monkeypatch.setattr(
        import_command,
        "_notify",
        lambda directory: notifications.append(directory),
    )

    import_command.import_cmd(
        directory=library,
        copy=False,
        link=False,
        nowrite=False,
        quiet=False,
        dry_run=False,
        config=None,
    )

    assert notifications == [library]


def test_import_rejects_missing_directory(tmp_path: Path) -> None:
    with pytest.raises(typer.Exit) as exc:
        import_command.import_cmd(
            directory=tmp_path / "missing",
            copy=False,
            link=False,
            nowrite=False,
            quiet=False,
            dry_run=False,
            config=None,
        )

    assert exc.value.exit_code == 1


def test_import_missing_config_uses_default_beets_config(
    tmp_path: Path,
    monkeypatch,
) -> None:
    library = tmp_path / "Library"
    library.mkdir()
    missing_config = tmp_path / "missing.yaml"
    calls = []

    monkeypatch.setattr(
        import_command,
        "import_paths",
        lambda options, *, decisions: calls.append(options),
    )
    monkeypatch.setattr(import_command, "_notify", lambda directory: None)

    import_command.import_cmd(
        directory=library,
        copy=False,
        link=False,
        nowrite=False,
        quiet=False,
        dry_run=False,
        config=missing_config,
    )

    assert calls[0].config_path is None

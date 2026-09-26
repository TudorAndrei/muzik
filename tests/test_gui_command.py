"""The CLI must launch the native app with its own Python interpreter."""

from __future__ import annotations

from pathlib import Path

import pytest

from muzik.commands import gui


def test_gui_launch_passes_active_python_to_native_app(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    binary = tmp_path / "muzik-gpui"
    binary.write_bytes(b"#!/bin/sh\n")
    binary.chmod(0o755)
    monkeypatch.setenv("MUZIK_GPUI_BIN", str(binary))
    calls: list[tuple[Path, list[str], dict[str, str]]] = []
    monkeypatch.setattr(
        gui.os,
        "execve",
        lambda path, argv, env: calls.append((path, argv, env)),
    )

    gui.gui_cmd()

    assert len(calls) == 1
    assert calls[0][0] == binary
    assert calls[0][1] == [str(binary)]
    assert calls[0][2]["MUZIK_PYTHON"] == gui.sys.executable


def test_gui_binary_rejects_missing_override(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("MUZIK_GPUI_BIN", "/no/such/muzik-gpui")

    with pytest.raises(FileNotFoundError, match="GPUI desktop binary is missing"):
        gui.gui_binary()

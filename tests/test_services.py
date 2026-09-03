import subprocess

from muzik.core import services
from muzik.core.services import (
    ServiceStatus,
    _check_binary,
    _check_chromium,
    _check_soulseek,
    check_services,
)


def test_check_binary_reports_missing(monkeypatch) -> None:
    monkeypatch.setattr(services.shutil, "which", lambda _: None)

    status = _check_binary("ffmpeg", "ffmpeg", ["-version"])

    assert status.available is False
    assert "PATH" in status.detail


def test_check_binary_reports_version(monkeypatch) -> None:
    monkeypatch.setattr(services.shutil, "which", lambda _: "/usr/bin/ffmpeg")
    monkeypatch.setattr(
        services.subprocess,
        "run",
        lambda *a, **k: subprocess.CompletedProcess(
            a, 0, "ffmpeg version 9.0\nrest", ""
        ),
    )

    status = _check_binary("ffmpeg", "ffmpeg", ["-version"])

    assert status.available is True
    assert status.detail == "ffmpeg version 9.0"


def test_check_binary_handles_run_failure(monkeypatch) -> None:
    monkeypatch.setattr(services.shutil, "which", lambda _: "/usr/bin/yt-dlp")

    def boom(*_a, **_k):
        raise OSError("denied")

    monkeypatch.setattr(services.subprocess, "run", boom)

    status = _check_binary("yt-dlp", "yt-dlp", ["--version"])

    assert status.available is False
    assert "failed to run" in status.detail


def test_check_chromium_uses_playwright_install_list(monkeypatch) -> None:
    monkeypatch.setattr(
        services.subprocess,
        "run",
        lambda *a, **k: subprocess.CompletedProcess(
            a,
            0,
            "Browsers:\n  /cache/ms-playwright/chromium-1234\n",
            "",
        ),
    )

    status = _check_chromium()

    assert status.available is True
    assert status.detail == "/cache/ms-playwright/chromium-1234"


def test_check_soulseek_reports_not_configured(monkeypatch) -> None:
    monkeypatch.setattr(
        services,
        "get_seakarr_settings",
        lambda: {"username": "", "password": "", "download_dir": "/tmp"},
    )

    status = _check_soulseek()

    assert status.available is None
    assert status.optional is True
    assert "configured" in status.detail


def test_check_services_returns_all_service_names() -> None:
    names = {status.name for status in check_services()}

    assert {
        "ffmpeg",
        "ffprobe",
        "yt-dlp",
        "Playwright Chromium",
        "Soulseek (Seakarr)",
    } <= names


def test_service_status_shape() -> None:
    status = ServiceStatus("x", True, "ok")

    assert status.optional is False

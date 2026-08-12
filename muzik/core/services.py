"""UI-neutral availability checks for external services and tools.

Both the CLI and the desktop interface can call :func:`check_services` to report
whether the binaries and services muzik depends on are reachable. Each check is
isolated: one failure never stops the others.
"""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
import shutil
import subprocess

from muzik.config import get_slskd_settings


@dataclass(frozen=True, slots=True)
class ServiceStatus:
    """Result of one service or tool check.

    ``available`` is ``True`` when ready, ``False`` when missing or failing, and
    ``None`` when an optional service is simply not configured.
    """

    name: str
    available: bool | None
    detail: str
    optional: bool = False


def check_services() -> list[ServiceStatus]:
    """Check every external tool and service muzik uses."""
    return [
        _check_binary("ffmpeg", "ffmpeg", ["-version"]),
        _check_binary("ffprobe", "ffprobe", ["-version"]),
        _check_binary("yt-dlp", "yt-dlp", ["--version"]),
        _check_chromium(),
        _check_slskd(),
    ]


def _check_binary(name: str, executable: str, version_args: list[str]) -> ServiceStatus:
    path = shutil.which(executable)
    if path is None:
        return ServiceStatus(name, False, f"Not found on PATH (install {executable}).")
    try:
        result = subprocess.run(
            [executable, *version_args],
            capture_output=True,
            text=True,
            timeout=10,
        )
    except (OSError, subprocess.SubprocessError) as exc:
        return ServiceStatus(name, False, f"Found at {path} but failed to run: {exc}")
    output = (result.stdout or result.stderr).strip().splitlines()
    version = output[0].strip() if output else path
    return ServiceStatus(name, True, version)


def _check_chromium() -> ServiceStatus:
    name = "Playwright Chromium"
    try:
        from playwright.sync_api import sync_playwright
    except ImportError:
        return ServiceStatus(name, False, "playwright is not installed.", optional=True)
    try:
        with sync_playwright() as playwright:
            executable = playwright.chromium.executable_path
    except Exception as exc:  # noqa: BLE001 - report any driver failure as unavailable
        return ServiceStatus(
            name, False, f"Playwright driver error: {exc}", optional=True
        )
    if executable and Path(executable).exists():
        return ServiceStatus(name, True, executable, optional=True)
    return ServiceStatus(
        name,
        False,
        "Not installed. Run: playwright install chromium",
        optional=True,
    )


def _check_slskd() -> ServiceStatus:
    name = "slskd (Soulseek)"
    settings = get_slskd_settings()
    if not settings["api_key"]:
        return ServiceStatus(
            name,
            None,
            "Not configured (set SLSKD_API_KEY).",
            optional=True,
        )
    try:
        from muzik.core.sources.soulseek import SoulseekSource

        info = SoulseekSource().check()
    except Exception as exc:  # noqa: BLE001 - any client error means unreachable
        return ServiceStatus(name, False, f"Unreachable: {exc}", optional=True)
    if not info.get("auth_valid"):
        return ServiceStatus(
            name, False, "Reachable but auth is invalid.", optional=True
        )
    if not (info.get("server_connected") and info.get("server_logged_in")):
        return ServiceStatus(
            name,
            False,
            "slskd is not logged in to Soulseek.",
            optional=True,
        )
    return ServiceStatus(name, True, f"Connected: {settings['url']}", optional=True)

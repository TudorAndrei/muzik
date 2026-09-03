"""UI-neutral availability checks for external services and tools.

Both the CLI and the desktop interface can call :func:`check_services` to report
whether the binaries and services muzik depends on are reachable. Each check is
isolated: one failure never stops the others.
"""

from __future__ import annotations

from dataclasses import dataclass
import shutil
import subprocess
import sys

from muzik.config import get_seakarr_settings


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
        _check_soulseek(),
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
        import playwright  # noqa: F401
    except ImportError:
        return ServiceStatus(name, False, "playwright is not installed.", optional=True)
    try:
        result = subprocess.run(
            [sys.executable, "-m", "playwright", "install", "--list"],
            capture_output=True,
            text=True,
            timeout=10,
            check=False,
        )
    except (OSError, subprocess.SubprocessError) as exc:
        return ServiceStatus(
            name, False, f"Playwright driver error: {exc}", optional=True
        )
    chromium = next(
        (
            line.strip()
            for line in result.stdout.splitlines()
            if "ms-playwright/chromium-" in line
            and "chromium_headless_shell" not in line
        ),
        None,
    )
    if result.returncode == 0 and chromium:
        return ServiceStatus(name, True, chromium, optional=True)
    return ServiceStatus(
        name,
        False,
        "Not installed. Run: playwright install chromium",
        optional=True,
    )


def _check_soulseek() -> ServiceStatus:
    name = "Soulseek (Seakarr)"
    settings = get_seakarr_settings()
    if not settings["username"] or not settings["password"]:
        return ServiceStatus(
            name,
            None,
            "Not configured (set MUZIK_SOULSEEK_USERNAME/MUZIK_SOULSEEK_PASSWORD).",
            optional=True,
        )
    try:
        from muzik.core.sources.seakarr import SeakarrSource

        info = SeakarrSource().check()
    except Exception as exc:  # noqa: BLE001 - any client error means unreachable
        return ServiceStatus(name, False, f"Unreachable: {exc}", optional=True)
    if not info.get("connected"):
        return ServiceStatus(
            name, False, info.get("detail", "Unreachable"), optional=True
        )
    return ServiceStatus(name, True, f"Connected: {info['server']}", optional=True)

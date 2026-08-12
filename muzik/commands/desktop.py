"""muzik install-app — create a macOS application bundle for the desktop UI.

The bundle is a thin wrapper: its launcher runs the already-installed
``muzik gui``. Finder-launched apps do not inherit the shell ``PATH``, so the
absolute path to the ``muzik`` executable is baked into the launcher script.
"""

from __future__ import annotations

from importlib import metadata, resources
from pathlib import Path
import os
import plistlib
import shutil
import subprocess
import sys
import tempfile

import typer

from muzik.ui.console import console


BUNDLE_NAME = "Muzik.app"
BUNDLE_ID = "com.tudorandrei.muzik"
LAUNCHER = "muzik-launcher"


def build_app_bundle(
    target_dir: Path,
    executable: str,
    *,
    icns: Path | None = None,
    version: str = "0.1.0",
) -> Path:
    """Write a ``Muzik.app`` bundle under ``target_dir`` and return its path."""
    app = target_dir / BUNDLE_NAME
    if app.exists():
        shutil.rmtree(app)
    macos = app / "Contents" / "MacOS"
    resources_dir = app / "Contents" / "Resources"
    macos.mkdir(parents=True)
    resources_dir.mkdir(parents=True)

    launcher = macos / LAUNCHER
    launcher.write_text(f'#!/bin/sh\nexec "{executable}" gui\n')
    launcher.chmod(0o755)

    info: dict[str, object] = {
        "CFBundleName": "Muzik",
        "CFBundleDisplayName": "Muzik",
        "CFBundleIdentifier": BUNDLE_ID,
        "CFBundleExecutable": LAUNCHER,
        "CFBundlePackageType": "APPL",
        "CFBundleVersion": version,
        "CFBundleShortVersionString": version,
        "LSMinimumSystemVersion": "13.0",
        "NSHighResolutionCapable": True,
    }
    if icns is not None and icns.exists():
        shutil.copyfile(icns, resources_dir / "muzik.icns")
        info["CFBundleIconFile"] = "muzik"

    with (app / "Contents" / "Info.plist").open("wb") as handle:
        plistlib.dump(info, handle)
    return app


def install_app_cmd(
    user: bool = typer.Option(
        False,
        "--user",
        help="Install to ~/Applications instead of /Applications.",
    ),
) -> None:
    """Create a macOS app bundle so muzik appears in Applications."""
    if sys.platform != "darwin":
        console.print("[red]install-app is only supported on macOS.[/red]")
        raise typer.Exit(1)

    executable = _resolve_muzik()
    target = _target_dir(user)
    target.mkdir(parents=True, exist_ok=True)

    icns = _prepare_icon()
    app = build_app_bundle(target, executable, icns=icns, version=_version())

    console.print(f"[green]Installed[/green] {app}")
    console.print(f"  Launches: [dim]{executable} gui[/dim]")
    if icns is None:
        console.print("  [yellow]No icon set (sips or logo unavailable).[/yellow]")
    console.print(
        "  Open it from Launchpad, Spotlight, or Finder. On first launch macOS "
        "may ask you to confirm an app from an unidentified developer."
    )


def _target_dir(user: bool) -> Path:
    if user:
        return Path.home() / "Applications"
    system = Path("/Applications")
    if os.access(system, os.W_OK):
        return system
    fallback = Path.home() / "Applications"
    console.print(
        f"[yellow]/Applications is not writable; using {fallback} instead.[/yellow]"
    )
    return fallback


def _resolve_muzik() -> str:
    found = shutil.which("muzik")
    if found:
        return found
    beside = Path(sys.executable).with_name("muzik")
    if beside.exists():
        return str(beside)
    return str(Path(sys.argv[0]).resolve())


def _version() -> str:
    try:
        return metadata.version("muzik")
    except metadata.PackageNotFoundError:
        return "0.1.0"


def _prepare_icon() -> Path | None:
    source = _source_icon()
    if source is None or shutil.which("sips") is None:
        return None
    work = Path(tempfile.mkdtemp(prefix="muzik-icon-"))
    png = work / "icon.png"
    icns = work / "muzik.icns"
    # icns needs a square source, so resample to 512x512 before converting.
    steps = (
        [
            "sips",
            "-s",
            "format",
            "png",
            "--resampleHeightWidth",
            "512",
            "512",
            str(source),
            "--out",
            str(png),
        ],
        ["sips", "-s", "format", "icns", str(png), "--out", str(icns)],
    )
    try:
        for step in steps:
            subprocess.run(step, check=True, capture_output=True)
    except OSError, subprocess.CalledProcessError:
        return None
    return icns if icns.exists() else None


def _source_icon() -> Path | None:
    try:
        packaged = resources.files("muzik").joinpath("assets/logo.jpeg")
        if packaged.is_file():
            return Path(str(packaged))
    except ModuleNotFoundError, FileNotFoundError, TypeError:
        pass
    repo = Path(__file__).resolve().parents[2] / "assets" / "logo.jpeg"
    return repo if repo.exists() else None

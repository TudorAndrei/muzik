import plistlib
import stat

from muzik.commands.desktop import build_app_bundle


def test_build_app_bundle_creates_launcher_and_plist(tmp_path) -> None:
    app = build_app_bundle(tmp_path, "/opt/homebrew/bin/muzik", version="1.2.3")

    assert app == tmp_path / "Muzik.app"
    launcher = app / "Contents" / "MacOS" / "muzik-launcher"
    assert launcher.read_text() == '#!/bin/sh\nexec "/opt/homebrew/bin/muzik" gui\n'
    assert launcher.stat().st_mode & stat.S_IXUSR

    info = plistlib.loads((app / "Contents" / "Info.plist").read_bytes())
    assert info["CFBundleExecutable"] == "muzik-launcher"
    assert info["CFBundleIdentifier"] == "com.tudorandrei.muzik"
    assert info["CFBundleShortVersionString"] == "1.2.3"
    assert "CFBundleIconFile" not in info


def test_build_app_bundle_includes_icon_when_given(tmp_path) -> None:
    icns = tmp_path / "src.icns"
    icns.write_bytes(b"icns-bytes")

    app = build_app_bundle(tmp_path / "out", "/usr/local/bin/muzik", icns=icns)

    assert (app / "Contents" / "Resources" / "muzik.icns").read_bytes() == b"icns-bytes"
    info = plistlib.loads((app / "Contents" / "Info.plist").read_bytes())
    assert info["CFBundleIconFile"] == "muzik"


def test_build_app_bundle_replaces_existing(tmp_path) -> None:
    build_app_bundle(tmp_path, "/first/muzik")
    app = build_app_bundle(tmp_path, "/second/muzik")

    launcher = app / "Contents" / "MacOS" / "muzik-launcher"
    assert '"/second/muzik"' in launcher.read_text()

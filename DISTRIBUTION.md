# Distribution

`muzik` has a Python 3.14 command-line interface, a Rust GPUI Kit desktop
program, and a PyO3 extension for Soulseek. The desktop program starts the
installed Python package as a child process. It uses the JSON protocol in
[muzik/native_gui/PROTOCOL.md](muzik/native_gui/PROTOCOL.md).

## Release files

The release workflow builds these files:

| File | Target | Contents |
| --- | --- | --- |
| macOS wheel | macOS arm64, Python 3.14 | Python package, Seakarr extension, GPUI app |
| Linux wheel | Linux x86_64, Python 3.14 | Python package, Seakarr extension, GPUI app |
| Source archive | Source | Python and both Rust crates |

The Linux wheel uses a `linux_x86_64` tag. It is built on Ubuntu with GPUI's
system libraries. It is not a manylinux wheel. Linux users need a working
display server and the shared libraries used by the GPUI binary. The release
workflow must test the binary on the target Linux system before the wheel is
called ready.

There is no Windows wheel in the current release workflow. The native app has
no signed installer or automatic updater.

## Build from the repository

Install the tools in `mise.toml`, then run:

```sh
mise install
uv sync --locked --dev
./scripts/build-native-gui.sh
uv build --wheel
python scripts/check-native-wheel.py
```

`scripts/build-native-gui.sh` builds `rust/gpui_app` and puts its executable in
`muzik/bin/`. Maturin builds `muzik._seakarr` from `rust/seakarr_bridge/` and
includes the executable in the wheel. The wheel check confirms the file and
its executable mode. Install a built wheel in a clean Python 3.14 environment
and run `muzik gui` to test the real application.

The source archive includes the GPUI crate but does not build its executable
when `pip` builds the Seakarr extension. A direct `pip install` from the source
archive gives the command-line interface; `muzik gui` then needs a separate
GPUI build. The Homebrew formula runs that build before `pip install`, so its
installed package contains the desktop program.

## Install paths

`uv tool install` installs a platform wheel as an isolated command. The
`muzik gui` command locates `muzik/bin/muzik-gpui` in that installation and
passes its Python interpreter path to the native program. In a source checkout,
the command can use a debug or release binary under `rust/gpui_app/target/`.
`MUZIK_GPUI_BIN` can name a specific executable.

On macOS, `muzik install-app` adds a small `Muzik.app` launcher to Applications.
It runs the installed `muzik gui` command. Re-run `install-app` after moving or
reinstalling the command. The app is not signed or notarized.

`ffmpeg`, `ffprobe`, and `yt-dlp` must be on `PATH`. Bandcamp needs a Playwright
Chromium installation and a browser login. Soulseek needs account credentials.
The CLI can run without a display; the GPUI app needs a GPU and display.

## Release checks

The release workflow sets one version in both Rust Cargo manifests, then
builds macOS and Linux wheels and a source archive. Each wheel job builds the
GPUI binary first, installs the wheel, and checks the Python service link.
The normal check workflow runs Python and Rust checks on Linux and a GPUI
build on macOS. This service check does not open a window.

Before publishing a desktop release, install each wheel in a clean environment
and check `muzik gui`, the Python service link, keyboard input, focus, workflow
decisions, cancellation, watchlist state, Spotify login, and window resizing on
that target. A successful compile or a wheel file check does not prove those
runtime behaviors.

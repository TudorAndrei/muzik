# Distribution and packaging investigation

This document examines how to distribute and package `muzik`. It records the
constraints, the realistic options, a recommendation, and the code changes that
must come first.

## What makes muzik hard to package

`muzik` is a Python 3.14 application with a DearPyGui desktop front end and a Typer
CLI. Four runtime facts drive every packaging choice:

1. **Required external binaries.** The code shells out to `ffmpeg`, `ffprobe`, and
   `yt-dlp` by name, resolved through `PATH` (37 `yt-dlp` call sites, plus `ffmpeg`
   and `ffprobe`). These are not Python packages.
2. **A browser automation dependency.** Bandcamp downloads launch Chromium through
   Playwright with `headless=False`. This needs a real browser and a display.
3. **An optional network service.** Soulseek support talks to a separate `slskd`
   server over HTTP. `muzik` does not own that process.
4. **Two interface modes.** The GUI needs a GPU and a display. The CLI runs
   headless.

`yt-dlp` also changes often, because it tracks site changes. A pinned, bundled
copy goes stale and breaks downloads. This argues against freezing `yt-dlp` into a
binary.

## Options examined

### Option A — Python wheel in GitHub Releases, installed with `uv tool` (selected)

Publish `muzik` as a wheel attached to a GitHub Release. Users install it as an
isolated application:

```sh
uv tool install \
  https://github.com/TudorAndrei/muzik/releases/download/v0.1.0/muzik-0.1.0-py3-none-any.whl
```

`uv tool` creates a private virtual environment and exposes the `muzik` console
script (already declared in `pyproject.toml`). The DearPyGui 2.3.1 wheel supplies
`cp314` builds for macOS arm64, Windows x86-64, and Linux, so the Python side
installs cleanly.

- **Pro:** lowest maintenance; cross-platform; matches the power-user audience;
  no code signing; upgrades with one command.
- **Con:** the user must install `ffmpeg`, `ffprobe`, and `yt-dlp` themselves, and
  run `playwright install chromium` once for Bandcamp.
- **Mitigation:** a `muzik doctor` / `muzik init` command that checks for the
  binaries and offers to run `playwright install`. A short prerequisite line:
  `brew install ffmpeg yt-dlp` (macOS) or `apt install ffmpeg` plus a `yt-dlp`
  install (Linux).

### Option B — Homebrew tap and formula (recommended secondary, macOS/Linux)

Ship a formula in a `homebrew-muzik` tap. Homebrew guidance for Python apps says
to install the app into `libexec` with its own virtual environment, bundle all
Python dependencies, and declare system dependencies explicitly.

```ruby
depends_on "python@3.14"
depends_on "ffmpeg"
depends_on "yt-dlp"
```

- **Pro:** one command installs the app and every external binary; the cleanest
  "it just works" path on macOS and Linux.
- **Con:** more maintenance (formula updates per release); Homebrew only.
  Chromium still needs a first-run `playwright install`.

### Option C — Frozen desktop bundle: PyInstaller, py2app, or Briefcase (not recommended as primary)

Build a `.app`/`.dmg` (macOS), `.msi` (Windows), or AppImage (Linux).

- DearPyGui freezes with PyInstaller, but needs
  `--hidden-import dearpygui.dearpygui` and a matching virtual environment.
- **Playwright is the blocker.** Bundling Chromium into a frozen app is fragile.
  On macOS it fails with "bundle format unrecognized". The accepted workaround is
  to *not* bundle the browser and instead download it on first run into a user
  directory through `PLAYWRIGHT_BROWSERS_PATH`. So a bundle still needs the
  first-run download anyway.
- Bundling `ffmpeg` adds license weight (GPL/LGPL) and size. Bundling `yt-dlp`
  goes stale.
- macOS needs code signing and notarization, or the app will not open.

Verdict: only worth it for non-technical users who never open a terminal. That is
not this tool's audience. Keep it as a possible later track, not the first one.

### Option D — Docker image (headless CLI only)

A `docker-compose.yml` already exists for the `slskd` side. A container can run the
headless CLI paths, but the DearPyGui GUI and the `headless=False` Bandcamp browser
do not run in a plain container without X forwarding.

Verdict: useful only for server-side, headless CLI use. Not a GUI distribution
path.

### Option E — conda / pixi (niche)

`conda-forge` supplies `ffmpeg` and `yt-dlp`, so a `pixi` environment can pull the
binaries and Python together. Useful for users already in that ecosystem, but a
smaller audience than PyPI or Homebrew.

## Decisions (resolved)

- **Target: macOS arm64 first.** One platform for the first release. Linux and
  Windows come later.
- **Assume the external binaries are on `PATH`.** `ffmpeg`, `ffprobe`, and `yt-dlp`
  are the user's responsibility. `muzik` does not bundle, pin, or relocate them. So
  no binary-path configuration work is needed for this release.
- **Track: Option A — GitHub Releases, installed with `uv tool`.** This is the
  first and only release track for now. No PyPI upload, Homebrew tap, frozen
  bundle, or Docker image in this round.

## Release plan (macOS arm64, GitHub Releases)

The build backend (`hatchling`) and the `muzik` console script are already in
`pyproject.toml`. The remaining work is small.

1. **Set the release metadata.** Use `0.1.0` as the first release version. Add
   `description`, `readme`, `license`, `authors`, classifiers, and a
   `requires-python` that matches (`>=3.14`).
2. **Confirm the DearPyGui wheel resolves for cp314 on macOS arm64.** Run
   `uv build` and `uv tool install --from ./dist/muzik-<version>-py3-none-any.whl
   muzik`, then start `muzik gui` and one CLI command in a clean shell.
3. **Document the prerequisites in the README.** State that macOS arm64 users must
   have `ffmpeg`, `ffprobe`, and `yt-dlp` on `PATH` (for example
   `brew install ffmpeg yt-dlp`), and must run `uv run playwright install chromium`
   once before Bandcamp use.
4. **Build and publish.** Add a GitHub Actions job that runs on a version tag,
   builds the wheel and source archive with `uv build`, and creates a GitHub
   Release with both files. The job uses the repository token and no stored
   release token.
5. **Verify the published install.** From a clean machine or shell, install the
   release wheel URL with `uv tool install`, then run `muzik gui` and a headless
   CLI command.

## Implementation status

- [x] Package metadata records version `0.1.0`, the README, author, Python 3.14,
  classifiers, repository links, and the current all-rights-reserved license.
- [x] The README states the macOS arm64 command prerequisites and the Playwright
  Chromium setup command.
- [x] `.github/workflows/release.yml` builds and publishes tag releases to
  GitHub without a stored token.
- [x] A clean Python 3.14 `uv tool` install passes for the built wheel. The
  isolated environment resolved DearPyGui 2.3.1, `muzik --help` passed, and
  `muzik gui` opened a viewport.
- [ ] Tag `v0.1.0` is pushed and the GitHub Release contains the wheel and source
  archive.
- [ ] The published wheel URL passes the install and command checks.

## Optional, not required for this release

- **`muzik doctor` command.** Checks `ffmpeg`, `ffprobe`, `yt-dlp`, and the
  Playwright browser and prints exact install commands. Good quality-of-life, but
  the "binaries on `PATH`" decision makes it optional.
- **Playwright browser location.** Keep the current first-run
  `playwright install chromium` flow. Owning `PLAYWRIGHT_BROWSERS_PATH` under the
  `platformdirs` data dir is a later refinement, not a blocker.
- **Homebrew tap and frozen bundle.** Deferred to later platform rounds.

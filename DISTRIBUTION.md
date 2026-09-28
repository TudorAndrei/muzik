# Distribution

The Cargo workspace at the repository root builds two Rust programs:
`muzik` in `rust/cli_app` and `muzik-gpui` in `rust/gpui_app`.

## Check a release

Run the same gate used by CI:

```sh
mise run check
```

This checks format, Clippy, Rust tests, the release build, and Cargo licenses.
The CLI and app read existing Beets config files and SQLite libraries. Check
both programs against copies of real Beets data before a public release.

## Create a release

The `Release` GitHub Actions workflow runs by request. Cocogitto reads
Conventional Commits and selects the next version. The `muzik-release` hook
sets each Cargo package version and updates `Cargo.lock`. The workflow builds
the CLI and GPUI app for macOS arm64, macOS x86_64, and Linux x86_64. Each
archive contains both programs, their license files, and the crate notices.

The two binaries must stay in the same directory so `muzik gui` can find the
desktop app. The Rust `yt-dlp` crate starts the `yt-dlp` program. Install that
program and `ffmpeg` on `PATH`. Install the upstream Rust
`bandsnatch` program to use the Bandcamp command.

The Homebrew formula in `packaging/homebrew` builds the Rust programs from
`main` with `brew install --HEAD`. Add a stable source URL and hash to the
external tap after the first Rust release.

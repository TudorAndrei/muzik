# Distribution

The Cargo workspace at the repository root builds two Rust programs:
`muzik` in `apps/cli` and `muzik-gpui` in `apps/gui`.

## Check a release

Run the complete gate:

```sh
mise run check-release
```

This checks format, Clippy, Rust tests, and Cargo licenses, then builds the
release binaries. CI runs `mise run check` on each push. The `Release` workflow
starts only after that check passed on the same commit.
The CLI and app read existing Beets config files and SQLite libraries. Check
both programs against copies of real Beets data before a public release.

## Create a release

The `Release` GitHub Actions workflow runs by request. Cocogitto reads
Conventional Commits and selects the next version. The `muzik-release` hook
sets each Cargo package version and updates `Cargo.lock`. The workflow builds
for macOS arm64 and Linux x86_64 and publishes:

- `muzik-cli-<tag>-<target>.tar.gz`: the command-line program only.
- `Muzik-<tag>-aarch64-apple-darwin.zip`: `Muzik.app`, built by
  `packaging/macos/build-app.sh`.
- `muzik-gpui-<tag>-x86_64-unknown-linux-gnu.tar.gz`: the Linux desktop program.

Each archive has the license files and crate notices. Both programs are
licensed under GPL-3.0-only.

The CLI and the desktop app are separate programs. The app adds the Homebrew
and mise tool folders to its `PATH`, so it finds `ffmpeg` and `yt-dlp` when it
opens from Finder.

## Publish to Homebrew and mise

The `homebrew` job of the `Release` workflow runs after the release is
published. It sets `version` and the `sha256` of the `Muzik-…zip` file (from its
`.sha256` asset) in `packaging/homebrew/Casks/muzik.rb`, commits that file to
`main`, and copies it to the `TudorAndrei/homebrew-muzik` tap. The tap push uses
the `HOMEBREW_TAP_DEPLOY_KEY` secret, a deploy key with write access to the tap.
mise needs no change: it reads the `muzik-cli` archive of the latest release.

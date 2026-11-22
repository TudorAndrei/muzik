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
Conventional Commits and selects the next version. The bump hook runs
`cargo set-version` (cargo-edit) on the workspace version that all packages
inherit, and updates `Cargo.lock`. The workflow builds
for macOS arm64 and Linux x86_64 and publishes:

- `muzik-cli-<tag>-<target>.tar.gz`: the command-line program only.
- `Muzik-<tag>-aarch64-apple-darwin.zip`: `Muzik.app`, built by
  `packaging/macos/build-app.sh`.
- `muzik-gpui-<tag>-x86_64-unknown-linux-gnu.tar.gz`: the Linux desktop program.

Each archive has the license files and crate notices. Both programs are
licensed under GPL-3.0-only.

Each archive also has `THIRD_PARTY_LICENSES.md`. It holds the license text of
each third-party crate in the build. `mise run third-party-licenses` makes it
with `cargo about` from `about.toml` and `about.md.hbs`. Keep the `accepted` list in
`about.toml` the same as the `allow` list and exceptions in `deny.toml`.

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

## Nix flake

`flake.nix` builds `muzik`, `muzik-server`, and, on Linux, `muzik-gpui` from
source with `nix/package.nix`. The version comes from `Cargo.toml`, so a release
needs no change to the flake. Cargo dependencies come from `Cargo.lock`.

A Git dependency, such as the `bolero-generator-derive` patch in `Cargo.toml`,
also needs a hash in `cargoLock.outputHashes` in `nix/package.nix`. When you add
or change one, set its hash to `lib.fakeHash`, run `nix build`, and copy the hash
from the error.

Run `nix flake update` to move the flake to a newer `nixpkgs`. Its Rust must be at
least the `rust-version` in `Cargo.toml`.

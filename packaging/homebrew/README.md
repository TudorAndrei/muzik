# Homebrew tap for muzik

`muzik.rb` builds the Rust CLI and GPUI app on macOS. It installs `ffmpeg`,
`yt-dlp`, and the upstream Rust `bandsnatch` program as dependencies. It does
not build or install Python.

The old stable formula points to a Python release. The new formula builds the
current `main` branch until a Rust release tag exists.

## Install from the Rust source

Copy this formula to `Formula/muzik.rb` in the `TudorAndrei/homebrew-muzik` tap.
Then run:

```sh
brew install --HEAD TudorAndrei/muzik/muzik
muzik --help
muzik gui
```

Homebrew builds both Rust programs from the locked Cargo workspace. Existing
Beets config files and SQLite library files remain in their normal data
locations. The formula does not change them.

## Add a stable version after the first Rust release

Add a `url` for the tagged GitHub source archive and its SHA-256 to the tap
formula. Keep the `head` entry for source builds. Check the new formula with
`brew audit --strict` and `brew test` before you publish it in the tap.
